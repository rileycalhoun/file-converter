mod templates;
mod endpoints;
mod schema;
mod models;

use std::{env, net::SocketAddr};

use anyhow::Result;
use dotenvy::dotenv;
use hyper::StatusCode;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt,util::SubscriberInitExt};
use axum::{async_trait, extract::{DefaultBodyLimit, FromRef, FromRequestParts}, http::request::Parts, routing::{get, post, Router}};
use tower_http::services::ServeDir;
use endpoints::{index::root, convert::convert, file::file, download::download};
use diesel_async::{
    pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection
};


type Pool = bb8::Pool<AsyncDieselConnectionManager<AsyncPgConnection>>;

struct DatabaseConnection(
    bb8::PooledConnection<'static, AsyncDieselConnectionManager<AsyncPgConnection>>,
);

#[async_trait]
impl<S> FromRequestParts<S> for DatabaseConnection
where 
    S: Send + Sync,
    Pool: FromRef<S>,
{

    type Rejection = (StatusCode, String);

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let pool = Pool::from_ref(state);
        let conn = pool.get_owned().await.map_err(internal_error)?;
        Ok(Self(conn))
    }

}

pub fn internal_error<E>(err: E) -> (StatusCode, String)
where
    E: std::error::Error,
{
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv().unwrap();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "RUST_LOG=debug".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
        )
        .init();

    let db_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set! Check your .env file!");

    let config = AsyncDieselConnectionManager::<AsyncPgConnection>::new(db_url);
    let pool = bb8::Pool::builder().build(config).await.unwrap();

    info!("Initializing service...");

    let app = Router::new()
        .route("/", get(root))
        .route("/convert", post(convert))
        .route("/files/:id", get(file))
        .route("/download/:id", get(download))
        .nest_service("/assets", ServeDir::new("static"))
        .layer(DefaultBodyLimit::max(20480 * 1024))
        .with_state(pool);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await?;

    info!("Service now listening at http://127.0.0.1:8000");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>()
    ).await?;
    
    Ok(())
}