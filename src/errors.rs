use hyper::StatusCode;

pub fn internal_error(err: ConverterError) -> (StatusCode, String) {
        match err {
            ConverterError::Convert(message) => (StatusCode::INTERNAL_SERVER_ERROR, message.into()),
            ConverterError::Download(message) => (StatusCode::INTERNAL_SERVER_ERROR, message.into()),         
            ConverterError::MissingDependencies(message) => (StatusCode::FAILED_DEPENDENCY, message.into()),
            ConverterError::DatabaseConnection(message) => (StatusCode::INTERNAL_SERVER_ERROR, message.into()),
        }
}

pub enum ConverterError<'de> {
    /// Internal Server Error, code 500
    /// Currently not in use.
    #[allow(dead_code)]
    Download(&'de str),
    
    /// Internal Server Error, code 500
    /// Same as 'DownloadError'
    Convert(&'de str),

    /// Failed Dependency Error, code 424
    MissingDependencies(&'de str),

    /// Internal Server Error, code 500
    DatabaseConnection(&'de str),
}