use std::net::SocketAddr;

use askama::Template;
use base64::{engine::general_purpose::STANDARD, Engine};
use diesel::{QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use tracing::debug;
use axum::{
    body::Body, extract::{ConnectInfo, Path}, http::{header, HeaderName}, response::{AppendHeaders, Html, IntoResponse}
};

use crate::{internal_error, models::File, templates::NotFound, DatabaseConnection};

pub enum DownloadResponse {
    Ok((AppendHeaders<Vec<(HeaderName, String)>>, Body)),
    NotFound
}

impl IntoResponse for DownloadResponse {

    fn into_response(self) -> axum::response::Response {
        match self {
            DownloadResponse::Ok((header, body)) => {
                (header, body).into_response()
            },
            DownloadResponse::NotFound => {
                let not_found = NotFound {};
                let html = Html(not_found.render().unwrap());
                html.into_response()
            }
        }
    }

}

pub async fn download(DatabaseConnection(mut conn): DatabaseConnection, ConnectInfo(addr): ConnectInfo<SocketAddr>, Path(identifier): Path<i32>) -> DownloadResponse {
    use crate::schema::files::dsl::*;
    let file: Result<File, _> = files
        .select(File::as_select())
        .find(identifier)
        .first(&mut conn)
        .await
        .map_err(internal_error);


    debug!("[{}] Attempting to find file {} in database!", addr, identifier);
    match file {
        Ok(file) => DownloadResponse::Ok(start_download(file.content, file.file_name)),
        Err(_) => DownloadResponse::NotFound
    }
}

fn start_download(base64: String, file_name: String) -> (AppendHeaders<Vec<(HeaderName, String)>>, Body) {
    let file_extension = &file_name[(file_name.rfind('.').unwrap_or(0) + 1)..file_name.len()];
    let mime_type = match file_extension {
        "pdf" => {
            "application/pdf"
        },
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        },
        "pptx" => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        _ => panic!("Could not download file of type {}", file_extension)
    };

    let headers: AppendHeaders<Vec<(HeaderName, String)>> = AppendHeaders([
        (header::CONTENT_TYPE, format!("{}; charset=utf-8", mime_type)),
        (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", file_name))
    ].to_vec());

    let bytes = STANDARD.decode(base64)
        .unwrap();
    let body: Body = Body::from(bytes);
    (headers, body)
}