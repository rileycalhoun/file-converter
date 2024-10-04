pub(crate) mod schema;
pub(crate) mod models;

use crate::SharedState;

use super::errors::{internal_error, ConverterError};
use axum::{async_trait, extract::FromRequestParts, http::request::Parts};
use diesel_async::{AsyncPgConnection, pooled_connection::AsyncDieselConnectionManager};
use hyper::StatusCode;

type Pool = bb8::Pool<AsyncDieselConnectionManager<AsyncPgConnection>>;

pub(crate) struct DatabaseConnection(
    pub bb8::PooledConnection<'static, AsyncDieselConnectionManager<AsyncPgConnection>>,
);

#[async_trait]
impl FromRequestParts<SharedState> for DatabaseConnection {

    type Rejection = (StatusCode, String);

    async fn from_request_parts(_parts: &mut Parts, state: &SharedState) -> Result<Self, Self::Rejection> {
        let state = state.read().await;

        let pool = &state.pool;
        let conn = pool.get_owned().await;

        if conn.is_err() {
            Err(internal_error(ConverterError::DatabaseConnection("Unable to connect to database!")))
        } else {
            let conn = conn.unwrap();
            Ok(Self(conn))
        }
    }

}
