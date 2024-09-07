use std::{env, net::SocketAddr};

use anyhow::Result;
use axum::{extract::{ConnectInfo, Multipart}, response::{IntoResponse, Redirect}};
use base64::{engine::general_purpose::STANDARD, Engine};
use diesel::SelectableHelper;
use diesel_async::RunQueryDsl;
use hyper::StatusCode;
use serde_json::json;
use tracing::{debug, info};

use crate::{database::DatabaseConnection, errors::{internal_error, ConverterError}, models::{File, NewFile}};

pub async fn convert(
    DatabaseConnection(mut conn): DatabaseConnection,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut form: Multipart
) -> Result<impl IntoResponse, (StatusCode, String)> {
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
        return Err(internal_error(ConverterError::MissingDependencies("You need to upload a file!")))
    }

    if conversion_type.is_none() {
        info!("[{}] Could not find conversion type...", addr);
        return Err(internal_error(ConverterError::MissingDependencies("You need to upload a file!")))
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
                            info!("[{}] Attempting to upload converted file to Postgres database!", addr);
                            let base64 = STANDARD.encode(file);

                            let new_file = NewFile {
                                file_name: &output_file_name,
                                content: &base64
                            };

                            let file = diesel::insert_into(crate::schema::files::table)
                                .values(&new_file)
                                .returning(File::as_returning())
                                .get_result(&mut conn)
                                .await;

                            match file {
                                Ok(file) => {
                                    info!("[{}] Redirecting to /files/{}", addr, file.id);
                                    Ok(Redirect::to(&format!("/files/{}", file.id)))
                                },
                                Err(_) =>  {
                                    info!("[{}] Recieved error while attempting to upload new file to Postgres!", addr);
                                    Err(internal_error(ConverterError::DatabaseConnection("Unable to connect to the database!")))
                                }
                            }
                        },
                        Err(err) => {
                            debug!("[{}] Error while reading bytes: {}", addr, err);
                            Err(internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!")))
                        }
                    } 
                }
                code => {
                    info!("[{}] Recieved the wrong status code from CloudConvert: {}", addr, code);
                    Err(internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!")))
                }
            }

        },
        Err(_) => {
            info!("[{}] Recieved error from CloudConvert job!", addr);
            Err(internal_error(ConverterError::Convert("Something went wrong while trying to convert the requested file!")))
        }
    }
}