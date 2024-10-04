pub(crate) mod index;
pub(crate) mod file;
pub(crate) mod download;
pub(crate) mod api;
pub(crate) mod webhooks;

use axum::{routing::get, Router};
use tower_http::services::ServeDir;

use crate::SharedState;
use self::{
    index::index,
    file::file,
    download::download
};

pub fn get_router() -> Router<SharedState> {
    Router::new()
        .route("/", get(index))
        .route("/files/:id", get(file))
        .route("/download/:id", get(download))
        .nest("/api", api::get_router())
        .nest_service("/assets", ServeDir::new("static"))
}
