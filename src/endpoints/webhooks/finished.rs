use std::collections::HashMap;

use axum::{extract::State, Json};
use tower_sessions::session::Id;
use tracing::{info, warn};

use crate::{
    database::DatabaseConnection,
    response::{
        AppResponse,
        JobResponse,
        JobTask
    },
    JobId,
    SharedState
};

async fn response(
    State(state): State<SharedState>,
    DatabaseConnection(_conn): DatabaseConnection,
    Json(body): Json<JobResponse>
) -> Json<AppResponse> {
    let job = body.job;
    let (id, _tag, tasks) = (job.id, job.tag, job.tasks);

    info!("[Job {}] Recieved completion response!", id);
    let job_id = JobId(id);

    let state = state.write().await;
    let pending_jobs = &state.pending_jobs;
    if !pending_jobs.contains_key(&job_id) {
        warn!("[Job {}] Job has no assigned session!", job_id.0);
        return Json(AppResponse {
            ok: false
        })
    }

    let _session_id = pending_jobs.get(&job_id).unwrap();
    let task = find_export_task(tasks);
    if task.is_none() {
        warn!("[Job {}] Webhook does not contain export-my-file task!", job_id.0);
        return Json(AppResponse {
            ok: false
        })
    }

    let _task = task.unwrap();

    return Json(AppResponse {
        ok: true
    })
}

fn find_export_task(tasks: Box<[JobTask]>) -> Option<JobTask> {
    for task in tasks {
        if task.name == "export-my-file" {
            return Some(task) 
        }
    }

    return None
}

fn still_waiting(pending_jobs: HashMap<JobId, Id>, id: Id) -> bool {
    for value in pending_jobs.values() {
        if *value == id {
            return true
        }
    }

    return false
}
