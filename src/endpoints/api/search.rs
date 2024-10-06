use std::net::SocketAddr;

use askama::Template;
use axum::{extract::ConnectInfo, response::{Html, IntoResponse, Response}, Form};
use diesel::{PgTextExpressionMethods, QueryDsl, SelectableHelper};
use diesel_async::RunQueryDsl;
use serde::Deserialize;
use tracing::info;

use crate::{database::{models::File, DatabaseConnection}, templates::SearchResults};
use crate::database::schema::files::dsl::{
    files as Files,
    file_name
};

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(rename = "search-term")]
    pub search_term: String
}

pub async fn search(
    DatabaseConnection(mut conn): DatabaseConnection,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Form(query): Form<SearchQuery>
) -> Response {
    info!("[{}] Recieved POST request on /api/search!", addr);


    let filter = format!("%{}%", &query.search_term);

    let files = Files
        .filter(file_name.ilike(filter))
        .select(File::as_select())
        .get_results(&mut conn)
        .await;

    let search_results = SearchResults {
        files: files.unwrap_or(Vec::new()),
        search_term: query.search_term
    };
    
    Html(search_results.render().unwrap()).into_response()
}
