mod cloudconvert;
mod config;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use uuid::Uuid;

use cloudconvert::CloudConvertClient;
use config::Config;

const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    auth_token: Arc<str>,
    cloudconvert: CloudConvertClient,
}

#[derive(Serialize)]
struct CreateConversionResponse {
    id: Uuid,
    status: &'static str,
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
        cloudconvert: CloudConvertClient::new(
            config.cloudconvert_api_key,
            config.cloudconvert_api_base,
        )?,
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_address)
        .await
        .context("failed to bind the gateway address")?;
    info!(address = %listener.local_addr()?, "CloudConvert gateway listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("gateway server stopped unexpectedly")
}

fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/conversions", post(create_conversion))
        .route("/v1/conversions/:id", get(conversion_status))
        .route("/v1/conversions/:id/download", get(download_conversion))
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

async fn create_conversion(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_name = None;
    let mut file_bytes = None;
    let mut output_format = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return bad_request(format!("invalid multipart upload: {error}")),
        };

        match field.name() {
            Some("file") => {
                file_name = field.file_name().map(sanitize_file_name);
                match field.bytes().await {
                    Ok(bytes) => file_bytes = Some(bytes),
                    Err(error) => {
                        return bad_request(format!("could not read uploaded file: {error}"))
                    }
                }
            }
            Some("output_format") => match field.text().await {
                Ok(value) => output_format = Some(value),
                Err(error) => return bad_request(format!("invalid output format: {error}")),
            },
            _ => {}
        }
    }

    let Some(file_name) = file_name else {
        return bad_request("the file must have a name");
    };
    let Some(file_bytes) = file_bytes else {
        return bad_request("a file is required");
    };
    let Some(output_format) = output_format else {
        return bad_request("an output format is required");
    };
    let output_format = output_format.trim().to_ascii_lowercase();
    if !is_valid_format(&output_format) {
        return bad_request("the output format must contain only letters and numbers");
    }

    match state
        .cloudconvert
        .create_job(&file_name, &file_bytes, &output_format)
        .await
    {
        Ok(id) => (
            StatusCode::ACCEPTED,
            Json(CreateConversionResponse {
                id,
                status: "processing",
            }),
        )
            .into_response(),
        Err(error) => upstream_error(error),
    }
}

async fn conversion_status(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match state.cloudconvert.job_status(id).await {
        Ok(status) => Json(status).into_response(),
        Err(error) => upstream_error(error),
    }
}

async fn download_conversion(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match state.cloudconvert.download(id).await {
        Ok(file) => {
            let disposition = format!("attachment; filename=\"{}\"", file.file_name);
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                Body::from(file.bytes),
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

fn is_valid_format(format: &str) -> bool {
    !format.is_empty()
        && format.len() <= 16
        && format
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
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
    warn!(%error, "CloudConvert request failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: "the conversion provider request failed".into(),
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
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::{is_valid_format, router, sanitize_file_name, AppState, CloudConvertClient};

    #[test]
    fn accepts_normal_output_formats() {
        assert!(is_valid_format("pdf"));
        assert!(is_valid_format("docx"));
        assert!(is_valid_format("jpg2000"));
    }

    #[test]
    fn rejects_format_injection() {
        assert!(!is_valid_format("../pdf"));
        assert!(!is_valid_format("pdf?x=1"));
        assert!(!is_valid_format(""));
    }

    #[test]
    fn strips_paths_from_upload_names() {
        assert_eq!(sanitize_file_name("../../secret.docx"), "secret.docx");
    }

    #[tokio::test]
    async fn conversion_routes_require_authentication() {
        let state = AppState {
            auth_token: Arc::from("test-token"),
            cloudconvert: CloudConvertClient::new(
                "unused-key".into(),
                "https://api.cloudconvert.com".into(),
            )
            .unwrap(),
        };
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/conversions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
