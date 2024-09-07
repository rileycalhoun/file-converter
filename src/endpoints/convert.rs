use std::{env, net::SocketAddr, time::Duration};

use anyhow::Result;
use axum::{extract::{ConnectInfo, Multipart}, response::{IntoResponse, Redirect}};
use base64::{engine::general_purpose::STANDARD, Engine};
use diesel::SelectableHelper;
use diesel_async::RunQueryDsl;
use hyper::StatusCode;
use serde_json::json;
use tokio::time::sleep;
use tracing::{debug, info};

use crate::{internal_error, models::{File, NewFile}, DatabaseConnection};

pub async fn convert(
    DatabaseConnection(mut conn): DatabaseConnection,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut form: Multipart
) -> Result<impl IntoResponse, StatusCode> {
    let mut input_file_name: Option<String> = None;
    let mut input_file_contents: Option<String> = None;
    let mut conversion_type: Option<String> = None;

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
        return Err(StatusCode::FAILED_DEPENDENCY)
    }

    if conversion_type.is_none() {
        info!("[{}] Could not find conversion type...", addr);
        return Err(StatusCode::FAILED_DEPENDENCY)
    }

    info!("[{}] POST request passed depenceny checks...", addr);

    let api_key = env::var("API_KEY")
        .expect("API_KEY must be set! Check your .env file!");

    let conversion_type = conversion_type.unwrap();
    let input_file_name = input_file_name.unwrap();

    let output_file_name = format!(
        "{}.{}",
        &input_file_name[0..input_file_name.rfind('.').unwrap_or(input_file_name.len())],
        conversion_type.to_lowercase() 
    );    

    info!("[{}] Starting POST request to CloudConvert...", addr);
    let client = reqwest::Client::new();
    let job_response = client.post("https://sync.api.cloudconvert.com/v2/jobs")
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
                StatusCode::OK => {
                    let file = job_response
                        .bytes()
                        .await;
                    match file {
                        Ok(file) => {
                            let base64 = STANDARD.encode(file);

                            let new_file = NewFile {
                                file_name: &output_file_name,
                                content: &base64
                            };

                            let file = diesel::insert_into(crate::schema::files::table)
                                .values(&new_file)
                                .returning(File::as_returning())
                                .get_result(&mut conn)
                                .await
                                .map_err(internal_error);

                            match file {
                                Ok(file) => {
                                    let _ = sleep(Duration::from_millis(10));
                                    info!("[{}] Redirecting to /files/{}", addr, file.id);
                                    Ok(Redirect::to(&format!("/files/{}", file.id)))
                                },
                                Err(_) =>  {
                                    info!("[{}] Recieved error while attempting to upload new file to Postgres!", addr);
                                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                                }
                            }
                        },
                        Err(err) => {
                            debug!("[{}] Error while reading bytes: {}", addr, err);
                            Err(StatusCode::from_u16(520).unwrap())
                        }
                    } 
                }
                code => {
                    info!("[{}] Recieved the wrong status code from CloudConvert: {}", addr, code);
                    Err(code)
                }
            }

        },
        Err(err) => {
            info!("[{}] Recieved error from CloudConvert job!", addr);
            match err.status() {
                Some(code) => {
                    debug!("[{}] Status Code: {}", addr, code);
                    Err(code)
                },
                None => {
                    debug!("[{}] Recieved no Status Code from CloudConvert!", addr);
                    Err(StatusCode::from_u16(520).unwrap())
                }
            }
        }
    }
}