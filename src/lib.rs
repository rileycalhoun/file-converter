pub mod templates;
pub mod endpoints;
pub mod response;
pub mod converter;
pub mod database;
pub mod webhook;
pub mod errors;

use converter::jobs::JobId;
use database::Pool;
use tokio::sync::{mpsc, RwLock};

use std::{collections::HashMap, sync::Arc};
use diesel_async::{pooled_connection::AsyncDieselConnectionManager, AsyncPgConnection};

pub enum JobStatus {
    PENDING, FAILED, COMPLETED
}

pub struct SocketMessage {
    job_status: JobStatus,
    file_id: Option<i32>,
    job_id: JobId
}

pub struct State {
    pool: Pool,
    pending_jobs: RwLock<HashMap<JobId, String>>,
    connected_clients: RwLock<HashMap<String, mpsc::Sender<SocketMessage>>>
}

impl State {
    pub async fn default(config: AsyncDieselConnectionManager<AsyncPgConnection>) -> State {
        State {
            pool: bb8::Pool::builder().build(config).await.unwrap(),
            pending_jobs: RwLock::new(HashMap::new()),
            connected_clients: RwLock::new(HashMap::new())
        }
    }

}

pub type SharedState = Arc<State>;
