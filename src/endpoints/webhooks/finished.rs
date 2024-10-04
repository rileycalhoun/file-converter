use axum::{extract::State, response::{IntoResponse, Response}, Json};
use tracing::{debug, error, info, warn};

use crate::{
    database::DatabaseConnection, response::{
        AppResponse,
        JobResponse,
        JobTask
    }, JobId, JobStatus, SharedState
};

pub async fn finished(
    State(state): State<SharedState>,
    DatabaseConnection(_conn): DatabaseConnection,
    json<JobResponse>: Json<JobResponse>
) -> Response {
    info!("");

    let job = body.job;
    let (id, _tag, tasks) = (job.id, job.tag, job.tasks);

    info!("[Job {}] Recieved completion response!", id);
    let job_id = JobId(id);

    let pending_jobs = &state.pending_jobs.read().await;
    if !pending_jobs.contains_key(&job_id) {
        warn!("[Job {}] Job has no assigned session!", job_id.0);
        return json(false)
    }

    let session_id = pending_jobs.get(&job_id).unwrap();
    let task = find_export_task(tasks);
    if task.is_none() {
        warn!("[Job {}] Webhook does not contain export-my-file task!", job_id.0);
        return json(false)
    }

    let task = task.unwrap();
    if task.percent != 100 {
        warn!("[Job {}] Recieved 'finished' payload but the task was still {} percent done!", job_id.0, task.percent);
        return json(false)
    }

    // Get file information, upload to database

    let clients = state.connected_clients.read().await; 
    if clients.contains_key(&session_id) {
        warn!("[Job {}] Ordering client is no longer connected!", job_id.0);
        return json(false)
    }

    let client = clients.get(&session_id).unwrap();
    if let Err(err) = client.send(crate::SocketMessage {
        job_status: JobStatus::COMPLETED,
        file_name: format!("Jared"),
        job_id: job_id.clone()
    }).await {
        error!("[Job {}] Recieved error while attempting to send message to client.", job_id.0);
        debug!("[Job {}] Error: {}", job_id.0, err);
        return json(false)
    }

    return json(true)
}

fn json(ok: bool) -> Response {
    Json(AppResponse {
        ok
    }).into_response()
} 

fn find_export_task(tasks: Box<[JobTask]>) -> Option<JobTask> {
    for task in tasks {
        if task.name == "export-my-file" {
            return Some(task) 
        }
    }

    return None
}
