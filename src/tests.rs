
#[test]
fn test_addr_variable() {
    use std::env;    

    dotenvy::from_filename(".env.example")
        .expect("Error while trying to load environment variables from .env.example!");
    let addr = env::var("ADDRESS")
        .expect("Error while trying to get ADDRESS from environment!");
    assert_eq!(addr, "0.0.0.0:8000")
}