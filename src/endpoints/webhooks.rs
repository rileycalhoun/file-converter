use axum::{Router,routing::post};

use crate::SharedState;

pub(crate) mod finished;
pub(crate) mod waiting;
pub(crate) mod failed;

pub(super) fn get_router() -> Router<SharedState> {
    Router::new()
        .route("/finished", post(finished::finished))
}
