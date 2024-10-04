use std::{env, net::SocketAddr};

use askama::Template;
use axum::{extract::ConnectInfo, response::Html};
use tower_sessions::Session;
use tracing::info;

use crate::templates::{Index, NotFound};

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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    session: Session
) -> Html<String> {
    info!("[{}] Recieved GET request on /", addr);

    if session.id().is_none() {
        if session
            .insert_value("mark_dirty", Default::default())
            .await
            .is_err()
        {
            return send_404()            
        }

        if session
            .remove_value("mark_dirty")
            .await
            .is_err()
        {
            return send_404()
        }

        if session
            .save()
            .await
            .is_err()
        {
            return send_404()
        }
    }

    let id = session.id().unwrap();

    let website_url = env::var("WEBSITE_URL").expect("Website URL must be set!");

    let index_template = Index {
        authorized_extensions: AUTHORIZED_EXTENSIONS.join(","),
        session_id: id.to_string(),
        website_url
    };

    Html(index_template.render().unwrap())
}

pub fn send_404() -> Html<String> {
    let not_found = NotFound {};
    Html(not_found.render().unwrap())
}
