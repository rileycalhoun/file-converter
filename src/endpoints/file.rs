use askama::Template;
use axum::{extract::Path, response::Html};
use diesel::{QueryDsl,  SelectableHelper};
use diesel_async::RunQueryDsl;

use crate::{internal_error, models::File, schema::files::dsl::*, templates::{FileInfo, NotFound}, DatabaseConnection};

pub async fn file(DatabaseConnection(mut conn): DatabaseConnection, Path(identifier): Path<i32>) -> Html<String> {
    // Find ID in Postgres database
    let file: Result<File, _> = files
        .select(File::as_select())
        .find(identifier)
        .first(&mut conn)
        .await
        .map_err(internal_error);

    match file {
        Ok(file) => {
            // If none, return 404
            // If found, return download page with sufficient information
            let file_info = FileInfo {
                download_uri: format!("/download/{identifier}"),
                file_name: file.file_name
            };

            Html(file_info.render().unwrap())
        },
        Err(_) => {
            let not_found = NotFound {};
            Html(not_found.render().unwrap())
        }
    }
}