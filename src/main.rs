mod templates;
mod endpoints;
mod response;
mod schema;
mod models;
mod database;
mod errors;

mod tests;

use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};

use anyhow::Result;
use dotenvy::dotenv;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_sessions::{cookie::time::Duration, session::Id, Expiry, MemoryStore, SessionManagerLayer};
use tracing::info;
use tower_http::services::ServeDir;
use endpoints::{index::root, file::file, download::download, api::{convert::convert, socket::socket}};
use tracing_subscriber::{layer::SubscriberExt,util::SubscriberInitExt};
use axum::{extract::{ws::WebSocket, DefaultBodyLimit}, routing::{get, post, Router}};
use diesel_async::{
    pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection
};

#[derive(Eq, Hash, PartialEq)]
pub struct JobId(String);
impl From<String> for JobId {
    
    fn from(value: String) -> Self {
        Self(value)
    }

}

impl Into<String> for JobId {

    fn into(self) -> String {
        self.0
    }
}

pub struct State {
    pool: bb8::Pool<AsyncDieselConnectionManager<AsyncPgConnection>>,
    connected_sockets: HashMap<Id, WebSocket>,
    pending_jobs: HashMap<JobId, Id>
}

impl State {
    async fn default(config: AsyncDieselConnectionManager<AsyncPgConnection>) -> State {
        State {
            pool: bb8::Pool::builder().build(config).await.unwrap(),
            connected_sockets: HashMap::new(),
            pending_jobs: HashMap::new()
        }
    }

}

pub type SharedState = Arc<Mutex<State>>;

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
    let shared_state: SharedState = Arc::new(
        Mutex::new(
            State::default(config).await
        )
    );

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(Duration::days(1)));

    info!("Initializing service...");
    
    let api = Router::new()
        .route("/convert", post(convert))
        .route("/socket", get(socket));

    let app = Router::new()
        .route("/", get(root))
        .route("/files/:id", get(file))
        .route("/download/:id", get(download))
        .nest("/api", api)
        .nest_service("/assets", ServeDir::new("static"))
        .layer(
            ServiceBuilder::new()
                .layer(DefaultBodyLimit::max(20480 * 1024))
                .layer(session_layer)
        )
        .with_state(shared_state);

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
