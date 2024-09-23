use std::collections::HashMap;

use axum::{extract::State, Json};
use tower_sessions::session::Id;
use tracing::{info, warn};

use crate::{database::DatabaseConnection, response::{AppResponse, JobResponse, JobTask}, JobId, SharedState};

async fn response(
    State(state): State<SharedState>,
    DatabaseConnection(conn): DatabaseConnection,
    Json(body): Json<JobResponse>
) -> Json<AppResponse> {
    let job = body.job;
    let (id, tag, tasks) = (job.id, job.tag, job.tasks);

    info!("[Job {}] Recieved completion reseponse!", id);
    let job_id = JobId(id);

    let state = state.lock().await;
    let mut pending_jobs = state.pending_jobs;
    if !pending_jobs.contains_key(&job_id) {
        warn!("[Job {}] Job has no assigned session!", id);
        return Json(AppResponse {
            ok: false
        })
    }

    let session_id = pending_jobs.get(&job_id).unwrap();
    let mut sockets = state.connected_sockets;
    if !sockets.contains_key(session_id) {
        warn!("[Job {}] Client disconnected before job was finished!", id);
        return Json(AppResponse {
            ok: false
        })
    }

    let task = find_export_task(tasks);
    if task.is_none() {
        warn!("[Job {}] Webhook does not contain export-my-file task!", id);
        return Json(AppResponse {
            ok: false
        })
    }

    let task = task.unwrap();
    //let files = task.;
    let mut socket = sockets.get(&session_id).unwrap();

    socket.send("completed;{}").await;

    return Json(AppResponse {
        ok: true
    })
}

fn find_export_task(tasks: Vec<JobTask>) -> Option<JobTask> {
    for task in tasks {
        if task.name == "export-my-file" {
            return Some(task) 
        }
    }

    return None
}

fn still_waiting(pending_jobs: HashMap<JobId, Id>, id: Id) -> bool {
    pending_jobs.contains_value(id)
}
