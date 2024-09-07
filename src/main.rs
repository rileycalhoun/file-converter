mod templates;
mod endpoints;
mod schema;
mod models;
mod database;
mod errors;

mod tests;

use std::{env, net::SocketAddr};

use anyhow::Result;
use dotenvy::dotenv;
use tower::ServiceBuilder;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt,util::SubscriberInitExt};
use axum::{extract::DefaultBodyLimit, routing::{get, post, Router}};
use tower_http::{trace::TraceLayer, services::ServeDir};
use endpoints::{index::root, convert::convert, file::file, download::download};
use diesel_async::{
    pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection
};




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
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(DefaultBodyLimit::max(20480 * 1024))
        )
        .with_state(pool);

    let addr = env::var("ADDRESS")
        .expect("ADDRESS must be set! Check your .env file!");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await?;

    info!("Service now listening at on {}", &addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>()
    ).await?;
    
    Ok(())
}