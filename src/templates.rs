use askama::Template;

#[derive(Template)]
#[template(path = "index.html")]
#[allow(dead_code)]
pub(crate) struct Index {
    pub(crate) authorized_extensions: String
}


#[derive(Template)]
#[template(path = "file.html")]
#[allow(dead_code)]
pub(crate) struct FileInfo {
    pub(crate) download_uri: String,
    pub(crate) file_name: String
}

#[derive(Template)]
#[template(path = "404.html")]
pub(crate) struct NotFound;