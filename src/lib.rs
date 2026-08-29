mod config;
mod gotenberg;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use config::Config;
use gotenberg::{output_file_name, GotenbergClient};

const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    auth_token: Arc<str>,
    gotenberg: GotenbergClient,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "file_converter_gateway=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let state = AppState {
        auth_token: Arc::from(config.gateway_token),
        gotenberg: GotenbergClient::new(config.gotenberg_url)?,
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_address)
        .await
        .context("failed to bind the gateway address")?;
    info!(address = %listener.local_addr()?, "Gotenberg gateway listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("gateway server stopped unexpectedly")
}

fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/convert", post(convert_file))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn convert_file(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_name = None;
    let mut file_bytes = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return bad_request(format!("invalid multipart upload: {error}")),
        };

        if let Some("file") = field.name() {
            file_name = field.file_name().map(sanitize_file_name);
            match field.bytes().await {
                Ok(bytes) => file_bytes = Some(bytes),
                Err(error) => return bad_request(format!("could not read uploaded file: {error}")),
            }
        }
    }

    let Some(file_name) = file_name else {
        return bad_request("the file must have a name");
    };
    let Some(file_bytes) = file_bytes else {
        return bad_request("a file is required");
    };
    match state.gotenberg.convert_to_pdf(&file_name, file_bytes).await {
        Ok(bytes) => {
            let disposition = format!("attachment; filename=\"{}\"", output_file_name(&file_name));
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/pdf".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                Body::from(bytes),
            )
                .into_response()
        }
        Err(error) => upstream_error(error),
    }
}

async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    if supplied == Some(state.auth_token.as_ref()) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "unauthorized".into(),
            }),
        )
            .into_response()
    }
}

fn sanitize_file_name(value: &str) -> String {
    std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload")
        .to_string()
}

fn bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

fn upstream_error(error: anyhow::Error) -> Response {
    warn!(%error, "Gotenberg conversion failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: "the document could not be converted to PDF".into(),
        }),
    )
        .into_response()
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install terminate handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        routing::post,
        Router,
    };
    use tower::ServiceExt;

    use super::{router, sanitize_file_name, AppState, GotenbergClient};

    #[test]
    fn strips_paths_from_upload_names() {
        assert_eq!(sanitize_file_name("../../secret.docx"), "secret.docx");
    }

    #[tokio::test]
    async fn conversion_routes_require_authentication() {
        let state = AppState {
            auth_token: Arc::from("test-token"),
            gotenberg: GotenbergClient::new("http://127.0.0.1:3000".into()).unwrap(),
        };
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[ignore = "requires permission to bind a loopback test server"]
    async fn conversion_route_returns_the_pdf_from_gotenberg() {
        let gotenberg = Router::new().route(
            "/forms/libreoffice/convert",
            post(|| async { ([(header::CONTENT_TYPE, "application/pdf")], "%PDF-test") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, gotenberg).await.unwrap();
        });

        let state = AppState {
            auth_token: Arc::from("test-token"),
            gotenberg: GotenbergClient::new(format!("http://{address}")).unwrap(),
        };
        let boundary = "file-converter-test-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"letter.docx\"\r\nContent-Type: application/octet-stream\r\n\r\ntest document\r\n--{boundary}--\r\n"
        );
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/convert")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"%PDF-test");
    }
}
