use askama::Template;

use crate::database::models::File;

#[derive(Template)]
#[template(path = "index.html")]
#[allow(dead_code)]
pub(crate) struct Index {
    pub(crate) authorized_extensions: String,
    pub(crate) session_id: String,
    pub(crate) website_url: String
}


#[derive(Template)]
#[template(path = "file.html")]
#[allow(dead_code)]
pub(crate) struct FileInfo {
    pub(crate) download_uri: String,
    pub(crate) file_name: String
}

#[derive(Template)]
#[template(path = "search/page.html")]
#[allow(dead_code)]
pub(crate) struct Search;

#[derive(Template)]
#[template(path = "search/results.html")]
#[allow(dead_code)]
pub(crate) struct SearchResults {
    pub files: Vec<File>,
    pub search_term: String
}

#[derive(Template)]
#[template(path = "404.html")]
pub(crate) struct NotFound;
