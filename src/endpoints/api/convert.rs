use std::{env, net::SocketAddr};

use axum::extract::{ConnectInfo, Multipart, State};
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::StatusCode;
use serde_json::json;
use tower_sessions::Session;
use tracing::{debug, info};

use crate::{
    converter::jobs::JobId, errors::{internal_error,ConverterError}, response::{CreateResponse, JobResponse}
};

pub async fn convert(
    session: Session,
    State(state): State<crate::SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut form: Multipart
) -> (StatusCode, String) {
    let mut input_file_name: Option<String> = None;
    let mut input_file_contents: Option<String> = None;
    let mut conversion_type: Option<String> = None;

    if session.id().is_none() {
        let _ = session.save().await;
    }

    let session_id = session.id().unwrap();

    info!("[{}] Recieved POST request on /convert", addr);

    while let Some(field) = form.next_field().await.unwrap() {
        let name = field.name().unwrap();
        match name {
            "input_file" => {
                input_file_name = Some(field.file_name().unwrap().to_string());
                let bytes = &field.bytes().await.unwrap().to_vec();
                input_file_contents = Some(STANDARD.encode(bytes));
            },
            "conversion_type" => conversion_type = Some(field.text().await.unwrap()),
            _ => continue,
        }
    }

    if input_file_contents.is_none() || input_file_name.is_none() {
        info!("[{}] Could not find input file...", addr);
        return internal_error(ConverterError::MissingDependencies("You need to upload a file!"))
    }

    if conversion_type.is_none() {
        info!("[{}] Could not find conversion type...", addr);
        return internal_error(ConverterError::MissingDependencies("You need to upload a file!"))
    }

    info!("[{}] POST request passed depenceny checks...", addr);

    let api_key = env::var("API_KEY")
        .expect("API_KEY must be set! Check your .env file!");

    let conversion_type = conversion_type.unwrap();
    let input_file_name = input_file_name.unwrap();

    info!("[{}] Starting POST request to CloudConvert...", addr);
    let client = reqwest::Client::new();

    let dev_mode = env::var("DEV_MODE")
        .expect("DEV_MODE must be set! Check your .env file!")
        .parse::<bool>()
        .expect("DEV_MODE must be true/false! Check your .env file!");

    let base_url = match dev_mode {
        false => env::var("CLOUDCONVERT_API").expect("CLOUDCONVERT_API must be set! Check your .env file!"),
        true  => env::var("CLOUDCONVERT_SANDBOX_API").expect("CLOUDCONVERT_SANDBOX_API must be set! Check your .env file!") 
    };

    let job_response = client.post(format!("{}/v2/jobs", base_url))
        .bearer_auth(&api_key)
        .json(&json!({
            "tasks": {
                "import-my-file": {
                    "operation": "import/base64",
                    "file": input_file_contents.unwrap(),
                    "filename": input_file_name,
                },

                "convert-my-file": {
                    "operation": "convert",
                    "input": "import-my-file",
                    "output_format": conversion_type,
                },

                "export-my-file": {
                    "operation": "export/url",
                    "input": "convert-my-file"
                }
            },
            "redirect": true
        }))
        .send()
        .await
        .unwrap();
    
    info!("[{}] Recieved response from CloudConvert!", addr);
    debug!("[{}] Response: {:?}", addr, job_response);
    match job_response.error_for_status() {
        Ok(job_response) => {
            match job_response.status() {
                StatusCode::CREATED => {
                    let bytes = job_response
                        .bytes()
                        .await;
                    match bytes {
                        Ok(bytes) => {
                            let response_string = String::from_utf8(bytes.to_vec()).unwrap();
                            debug!("{}", response_string);

                            let response: CreateResponse = serde_json::from_str(&response_string).unwrap();
                            let job_id = JobId::from(response.data.id);

                            let mut pending = state.pending_jobs.write().await;
                            pending.insert(job_id, session_id);
                            drop(pending);
                             
                            (StatusCode::OK, "You will be redirected when your file(s) have completed converting.".to_string())
                        },
                        Err(err) => {
                            debug!("[{}] Error while reading bytes: {}", addr, err);
                            internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!"))
                        }
                    } 
                }
                code => {
                    info!("[{}] Recieved the wrong status code from CloudConvert: {}", addr, code);
                    internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!"))
                }
            }

        },
        Err(_) => {
            info!("[{}] Recieved error from CloudConvert job!", addr);
            internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!"))
        }
    }
}
