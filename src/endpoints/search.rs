use askama::Template;
use axum::response::{Html, IntoResponse};

use crate::templates::Search;

pub async fn search() -> impl IntoResponse {
    let search = Search {};
    Html(search.render().unwrap())
}
