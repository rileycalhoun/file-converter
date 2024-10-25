use anyhow::Result;
use dotenvy::dotenv;
use fred::{prelude::{ClientLike, RedisPool}, types::RedisConfig};
use tower_sessions_redis_store::RedisStore;
use tracing::info;

use std::{env, net::SocketAddr, sync::Arc};
use tower_sessions::{cookie::time::{Duration, OffsetDateTime}, Expiry, SessionManagerLayer};
use tracing_subscriber::{layer::SubscriberExt,util::SubscriberInitExt};
use axum::extract::DefaultBodyLimit;
use diesel_async::{
    pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection
};
use file_converter::{
    endpoints::get_router, SharedState, State
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
    let shared_state: SharedState = Arc::new(
            State::default(config).await
    );

    let redis_url = env::var("REDIS_URL")
        .expect("REDIS_URL must be set! Check your .env file!");

    let redis_config = RedisConfig::from_url(&redis_url)
        .expect("Unable to connect to Redis server using provided details!");

    let redis_client = RedisPool::new(redis_config, None, None, None, 6)
        .expect("Unable to connect to Redis server using provided details!");

    let _conn = redis_client.connect();
    redis_client.wait_for_connect().await
        .expect("Unable to connect to Redis server using provided details!");

    let session_store = RedisStore::new(redis_client);
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false)
        .with_expiry(Expiry::AtDateTime(OffsetDateTime::now_utc().checked_add(Duration::days(1)).unwrap()));

    info!("Initializing service...");
    let app = get_router()
        .layer(DefaultBodyLimit::max(20480 * 1024))
        .layer(session_layer)
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
