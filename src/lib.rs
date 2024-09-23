pub mod templates;
pub mod endpoints;
pub mod response;
pub mod converter;
pub mod database;
pub mod webhook;
pub mod errors;
pub mod notifications;

use converter::jobs::JobId;
use tokio::sync::RwLock;
use tower_sessions::session::Id as SessionId;

use std::{collections::HashMap, sync::Arc};
use diesel_async::{pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection};

pub struct State {
    pool: bb8::Pool<AsyncDieselConnectionManager<AsyncPgConnection>>,
    pending_jobs: HashMap<JobId, SessionId>
}

impl State {
    pub async fn default(config: AsyncDieselConnectionManager<AsyncPgConnection>) -> State {
        State {
            pool: bb8::Pool::builder().build(config).await.unwrap(),
            pending_jobs: HashMap::new()
        }
    }

}

pub type SharedState = Arc<RwLock<State>>;
