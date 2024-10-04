use std::net::SocketAddr;

use askama::Template;
use axum::{extract::ConnectInfo, response::Html};
use tracing::info;

use crate::templates::Index;

const AUTHORIZED_EXTENSIONS: [&str; 8] = [
    ".jpg",
    ".jpeg",
    ".png",
    ".ppt",
    ".pptx",
    ".doc",
    ".docx",
    ".pdf"
];

pub async fn index(
    ConnectInfo(addr): ConnectInfo<SocketAddr>
) -> Html<String> {
    info!("[{}] Recieved GET request on /", addr);
    let index_template = Index {
        authorized_extensions: AUTHORIZED_EXTENSIONS.join(",")
    };

    Html(index_template.render().unwrap())
}
