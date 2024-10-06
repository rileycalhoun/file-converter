pub mod convert;
pub mod search;

use axum::{routing::post, Router};
use convert::convert;
use search::search;

pub fn get_router() -> Router<crate::SharedState> {
    Router::new()
        .route("/convert", post(convert))
        .route("/search", post(search))
}
