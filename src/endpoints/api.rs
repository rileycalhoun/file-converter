pub mod convert;

use axum::{routing::post, Router};
use convert::convert;

pub fn get_router() -> Router<crate::SharedState> {
    Router::new()
        .route("/convert", post(convert))
}
