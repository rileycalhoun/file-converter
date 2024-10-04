use axum::{extract::State, response::{IntoResponse, Response}, Json};
use diesel::SelectableHelper;
use diesel_async::RunQueryDsl;
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, warn};
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::{
    database::{models::{File, NewFile}, DatabaseConnection}, response::{
        Job,
        JobTask
    }, JobId, JobStatus, SharedState, SocketMessage
};

pub async fn finished(
    State(state): State<SharedState>,
    DatabaseConnection(mut conn): DatabaseConnection,
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

    let task = find_export_task(tasks);
    if task.is_none() {
        warn!("[Job {}] Webhook does not contain export-my-file task!", job_id.0);
        return json(false)
    }

    let task = task.unwrap();
    let file = task.result.files.get(0);
    let mut clients = state.connected_clients.write().await; 
    let client = clients.get(&session_id);
    if client.is_none() {
        warn!("[{}] Client is no longer connected!", job_id.0);
        return json(false)
    }

    let client = client.unwrap();

    let success = match file {
        Some(file) => {
            let url = file.url.clone();
            match url {
                Some(url) => {
                    let reqwest_client = reqwest::Client::new();
                    let response = reqwest_client.get(url)
                        .send()
                        .await
                        .unwrap();

                    match response.error_for_status() {
                        Ok(response) => {
                            let bytes = response.bytes().await;
                            match bytes {
                                Ok(bytes) => {
                                    let base64 = STANDARD.encode(bytes);
                                    let new_file = NewFile {
                                        file_name: &file.file_name,
                                        content: &base64
                                    };

                                    let file = diesel::insert_into(crate::database::schema::files::table)
                                        .values(&new_file)
                                        .returning(File::as_returning())
                                        .get_result(&mut conn)
                                        .await;

                                    match file {
                                        Ok(file) => {
                                            send_client_message(client, SocketMessage {
                                                job_id: job_id.clone(),
                                                job_status: JobStatus::COMPLETED,
                                                file_id: Some(file.id)
                                            }).await;
                                            
                                            true
                                        },
                                        Err(_) => {
                                            error!("[{}] There was an error while attempting to upload the file to the database!", job_id.0);
                                            false
                                        }
                                    }

                                },
                                Err(err) => {
                                    error!("[{}] Recieved error code when attempting to request file: {}", job_id.0, err);
                                    false
                                }
                            }
                        },
                        Err(_) => {
                            error!("[{}] Could not get converted file from given URL.", job_id.0);
                            false
                        }
                    }
                },
                None => {
                    error!("[{}] Could not find any specified URL from the task!", job_id.0);
                    false
                }
            }
        },
        None => {
            error!("[{}] Could not find any file in task!", job_id.0);
            false
        }
    };

    if success == false {
        send_client_message(client, SocketMessage {
            job_id,
            job_status: JobStatus::FAILED,
            file_id: None
        }).await;
    }

    clients.remove(&session_id);
    drop(clients);

    json(success)
}

async fn send_client_message(client: &Sender<SocketMessage>, msg: SocketMessage) -> bool {
    let job_id = msg.job_id.clone();
    if let Err(err) = client.send(msg).await {
        error!("[Job {}] Recieved error while attempting to send message to client.", job_id.0);
        debug!("[Job {}] Error: {}", job_id.0, err);
        return false
    }

    return true
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
