use axum::{extract::State, response::{IntoResponse, Response}, Json};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::{
    database::DatabaseConnection, response::{
        Job,
        JobTask
    }, JobId, JobStatus, SharedState
};

pub async fn finished(
    State(state): State<SharedState>,
    DatabaseConnection(_conn): DatabaseConnection,
    Json(body): Json<Value>
) -> Response {
    {
        let event = &body["event"];
        if event != &Value::String(format!("job.finished")) {
            warn!("Recieved {} event on /webhooks/finished", body["event"]); 
            return json(false)
        }

        info!("Recieved \"job.finished\" event!");
    }

    let job = body["job"].clone();
    let job = serde_json::from_value::<Job>(job);
    if let Err(err) = job {
        warn!("Recieved error when attempting to unwrap job: {}", err);
        return json(false);
    }

    let job = job.unwrap();
    let (id, tasks) = (job.id, job.tasks);

    info!("[Job {}] Recieved completion response!", id);
    let job_id = JobId(id);

    let task = find_export_task(tasks);
    if task.is_none() {
        warn!("[Job {}] Webhook does not contain export-my-file task!", job_id.0);
        return json(false)
    }

    let task = task.unwrap();
    let file = task.result.files.get(0).unwrap();

    let url = file.url.clone();
    let url = url.unwrap();

    let client = reqwest::Client::new();
    let request = client.get(url).build().unwrap();
    let response = client.execute(request).await;

    if response.is_err() {
        error!("[{}] Could not get converted file from given URL.", job_id.0);
        return json(false);
    }

    let response = response.unwrap();
    let body = response.bytes().await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    info!("{}", body);

    // Get file information, upload to database

    let pending_jobs = &state.pending_jobs.read().await;
    if !pending_jobs.contains_key(&job_id) {
        warn!("[{}] Job has no assigned session!", job_id.0);
        return json(false)
    }

    let session_id = pending_jobs.get(&job_id);
    if session_id.is_none() {
        warn!("[{}] Job is not in pending_jobs!", job_id.0);
        return json(false)
    }

    let session_id = session_id.unwrap().to_string();

    let mut clients = state.connected_clients.write().await; 
    let client = clients.get(&session_id);
    if client.is_none() {
        warn!("[{}] Client is no longer connected!", job_id.0);
        return json(false)
    }

    let client = client.unwrap();
    if let Err(err) = client.send(crate::SocketMessage {
        job_status: JobStatus::COMPLETED,
        file_name: format!("Jared"),
        job_id: job_id.clone()
    }).await {
        error!("[Job {}] Recieved error while attempting to send message to client.", job_id.0);
        debug!("[Job {}] Error: {}", job_id.0, err);
        return json(false)
    }

    clients.remove(&session_id);
    drop(clients);

    return json(true)
}

fn json(ok: bool) -> Response {
    let ok = json!(ok);
    let response = json!({
        "ok": ok
    });

    return Json(response).into_response()
} 

fn find_export_task(tasks: Vec<JobTask>) -> Option<JobTask> {
    for task in tasks {
        if task.operation == "export/url" {
            return Some(task) 
        }
    }

    return None
}
