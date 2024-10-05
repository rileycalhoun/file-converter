pub(crate) mod index;
pub(crate) mod file;
pub(crate) mod download;
pub(crate) mod search;
pub(crate) mod api;
pub(crate) mod webhooks;
pub(crate) mod websocket;

use axum::{routing::get, Router};
use tower_http::services::ServeDir;

use crate::SharedState;
use self::{
    index::index,
    file::file,
    download::download,
    search::search
};

pub fn get_router() -> Router<SharedState> {
    Router::new()
        .route("/", get(index))
        .route("/files/:id", get(file))
        .route("/download/:id", get(download))
        .route("/search", get(search))
        .route("/ws", get(websocket::socket))
        .nest("/api", api::get_router())
        .nest("/webhooks", webhooks::get_router())
        .nest_service("/assets", ServeDir::new("static"))
}
