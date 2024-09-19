
#[test]
fn test_addr_variable() {
    use std::env;    

    dotenvy::from_filename(".env.example")
        .expect("Error while trying to load environment variables from .env.example!");
    let addr = env::var("ADDRESS")
        .expect("Error while trying to get ADDRESS from environment!");
    assert_eq!(addr, "0.0.0.0:8000")
}

#[test]
fn test_errors() {
    use crate::errors::{internal_error, ConverterError};
    use hyper::StatusCode;

    let download_error = internal_error(ConverterError::Download("Unable to download file!"));
    assert_eq!(download_error, (StatusCode::INTERNAL_SERVER_ERROR, "Unable to download file!".into()));

    let convert_error = internal_error(ConverterError::Convert("Unable to convert file!"));
    assert_eq!(convert_error, (StatusCode::INTERNAL_SERVER_ERROR, "Unable to convert file!".into()));

    let database_error = internal_error(ConverterError::DatabaseConnection("Unable to connect to the database!"));
    assert_eq!(database_error, (StatusCode::INTERNAL_SERVER_ERROR, "Unable to connect to the database!".into()));

    let dependency_error = internal_error(ConverterError::MissingDependencies("You are missing required dependencies!"));
    assert_eq!(dependency_error, (StatusCode::FAILED_DEPENDENCY, "You are missing required dependencies!".into()));
}