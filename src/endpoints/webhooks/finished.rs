use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use diesel::SelectableHelper;
use diesel_async::RunQueryDsl;
use hmac::KeyInit;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, warn};

use crate::{
    database::{
        models::{File, NewFile},
        DatabaseConnection,
    },
    response::{Job, JobTask},
    JobId, JobStatus, SharedState, SocketMessage,
};

type HmacSha256 = Hmac<Sha256>;

fn verify_cloudconvert_signature(
    headers: &HeaderMap,
    body: &[u8],
    signing_secret: &str,
) -> Result<(), StatusCode> {
    let signature = headers
        .get("cloudconvert-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let signature = hex::decode(signature).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| StatusCode::UNAUTHORIZED)
}

pub async fn finished(
    State(state): State<SharedState>,
    DatabaseConnection(mut conn): DatabaseConnection,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let signing_secret = match std::env::var("CLOUDCONVERT_WEBHOOK_SECRET") {
        Ok(secret) => secret,
        Err(_) => {
            error!("CLOUDCONVERT_WEBHOOK_SECRET is not configured");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(status) = verify_cloudconvert_signature(&headers, &body, &signing_secret) {
        warn!("Rejected webhook with invalid signature");
        return status.into_response();
    }

    let body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            warn!("Invalid webhook JSON: {error}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let event = &body["event"];
    if event != &Value::String(format!("job.finished")) {
        warn!("Recieved {} event on /webhooks/finished", body["event"]);
        return json(false);
    }

    info!("Recieved \"job.finished\" event!");

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
        return json(false);
    }

    let session_id = pending_jobs.get(&job_id);
    if session_id.is_none() {
        warn!("[{}] Job is not in pending_jobs!", job_id.0);
        return json(false);
    }

    let session_id = session_id.unwrap().to_string();

    let task = find_export_task(tasks);
    if task.is_none() {
        warn!(
            "[Job {}] Webhook does not contain export-my-file task!",
            job_id.0
        );
        return json(false);
    }

    let task = task.unwrap();
    let file = task.result.files.get(0);
    let mut clients = state.connected_clients.write().await;
    let client = clients.get(&session_id);
    if client.is_none() {
        warn!("[{}] Client is no longer connected!", job_id.0);
        return json(false);
    }

    let client = client.unwrap();

    let success = match file {
        Some(file) => {
            let url = file.url.clone();
            match url {
                Some(url) => {
                    let reqwest_client = reqwest::Client::new();
                    let response = reqwest_client.get(url).send().await.unwrap();

                    match response.error_for_status() {
                        Ok(response) => {
                            let bytes = response.bytes().await;
                            match bytes {
                                Ok(bytes) => {
                                    let base64 = STANDARD.encode(bytes);
                                    let new_file = NewFile {
                                        file_name: &file.file_name,
                                        content: &base64,
                                    };

                                    let file =
                                        diesel::insert_into(crate::database::schema::files::table)
                                            .values(&new_file)
                                            .returning(File::as_returning())
                                            .get_result(&mut conn)
                                            .await;

                                    match file {
                                        Ok(file) => {
                                            send_client_message(
                                                client,
                                                SocketMessage {
                                                    job_id: job_id.clone(),
                                                    job_status: JobStatus::COMPLETED,
                                                    file_id: Some(file.id),
                                                },
                                            )
                                            .await;

                                            true
                                        }
                                        Err(_) => {
                                            error!("[{}] There was an error while attempting to upload the file to the database!", job_id.0);
                                            false
                                        }
                                    }
                                }
                                Err(err) => {
                                    error!("[{}] Recieved error code when attempting to request file: {}", job_id.0, err);
                                    false
                                }
                            }
                        }
                        Err(_) => {
                            error!(
                                "[{}] Could not get converted file from given URL.",
                                job_id.0
                            );
                            false
                        }
                    }
                }
                None => {
                    error!(
                        "[{}] Could not find any specified URL from the task!",
                        job_id.0
                    );
                    false
                }
            }
        }
        None => {
            error!("[{}] Could not find any file in task!", job_id.0);
            false
        }
    };

    if success == false {
        send_client_message(
            client,
            SocketMessage {
                job_id,
                job_status: JobStatus::FAILED,
                file_id: None,
            },
        )
        .await;
    }

    clients.remove(&session_id);
    drop(clients);

    json(success)
}

async fn send_client_message(client: &Sender<SocketMessage>, msg: SocketMessage) -> bool {
    let job_id = msg.job_id.clone();
    if let Err(err) = client.send(msg).await {
        error!(
            "[Job {}] Recieved error while attempting to send message to client.",
            job_id.0
        );
        debug!("[Job {}] Error: {}", job_id.0, err);
        return false;
    }

    return true;
}

fn json(ok: bool) -> Response {
    let ok = json!(ok);
    let response = json!({
        "ok": ok
    });

    return Json(response).into_response();
}

fn find_export_task(tasks: Vec<JobTask>) -> Option<JobTask> {
    for task in tasks {
        if task.operation == "export/url" {
            return Some(task);
        }
    }

    return None;
}

#[cfg(test)]
mod tests {
    use super::verify_cloudconvert_signature;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    const SIGNING_SECRET: &str = "test-signing-secret";
    const BODY: &[u8] = br#"{"event":"job.finished","job":{"id":"job-123"}}"#;

    // Generated independently with:
    // printf '%s' '<BODY>' | openssl dgst -sha256 -hmac 'test-signing-secret'
    const SIGNATURE: &str = "6551b4fcdc20354f02f5dc0068b09e2489c4ab6955eeefaba53395ecf5b9327b";

    fn headers_with_signature(signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cloudconvert-signature",
            HeaderValue::from_str(signature)
                .expect("test signature should be a valid header value"),
        );
        headers
    }

    #[test]
    fn accepts_a_valid_signature_for_the_exact_raw_body() {
        let headers = headers_with_signature(SIGNATURE);

        assert_eq!(
            verify_cloudconvert_signature(&headers, BODY, SIGNING_SECRET),
            Ok(())
        );
    }

    #[test]
    fn rejects_a_signature_when_the_body_is_modified() {
        let headers = headers_with_signature(SIGNATURE);
        let modified_body = br#"{"event":"job.failed","job":{"id":"job-123"}}"#;

        assert_eq!(
            verify_cloudconvert_signature(&headers, modified_body, SIGNING_SECRET),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_a_signature_when_raw_body_formatting_changes() {
        let headers = headers_with_signature(SIGNATURE);
        let body_with_trailing_newline = br#"{"event":"job.finished","job":{"id":"job-123"}}
"#;

        assert_eq!(
            verify_cloudconvert_signature(&headers, body_with_trailing_newline, SIGNING_SECRET),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_an_incorrect_signature() {
        let headers = headers_with_signature(
            "0000000000000000000000000000000000000000000000000000000000000000",
        );

        assert_eq!(
            verify_cloudconvert_signature(&headers, BODY, SIGNING_SECRET),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_a_missing_signature_header() {
        assert_eq!(
            verify_cloudconvert_signature(&HeaderMap::new(), BODY, SIGNING_SECRET),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_a_non_hex_signature() {
        let headers = headers_with_signature("not-a-hex-signature");

        assert_eq!(
            verify_cloudconvert_signature(&headers, BODY, SIGNING_SECRET),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
