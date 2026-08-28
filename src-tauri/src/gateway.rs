use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{multipart, Client, StatusCode};
use serde::Deserialize;
use uuid::Uuid;

use crate::database::PrivateSettings;

pub struct GatewayClient {
    client: Client,
}

#[derive(Deserialize)]
struct CreateResponse {
    id: Uuid,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum RemoteStatus {
    Processing,
    Finished { file_name: String },
    Failed { message: String },
}

impl GatewayClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()
                .context("could not create the gateway HTTP client")?,
        })
    }

    pub async fn create_conversion(
        &self,
        settings: &PrivateSettings,
        file_name: &str,
        bytes: Vec<u8>,
        output_format: &str,
    ) -> Result<Uuid> {
        let file = multipart::Part::bytes(bytes).file_name(file_name.to_string());
        let form = multipart::Form::new()
            .part("file", file)
            .text("output_format", output_format.to_string());
        let response = self
            .client
            .post(format!("{}/v1/conversions", settings.gateway_url))
            .bearer_auth(&settings.gateway_token)
            .multipart(form)
            .send()
            .await
            .context("Could not reach the conversion gateway.")?;
        decode_json(response)
            .await
            .map(|value: CreateResponse| value.id)
    }

    pub async fn conversion_status(
        &self,
        settings: &PrivateSettings,
        id: Uuid,
    ) -> Result<RemoteStatus> {
        let response = self
            .client
            .get(format!("{}/v1/conversions/{id}", settings.gateway_url))
            .bearer_auth(&settings.gateway_token)
            .send()
            .await
            .context("Could not reach the conversion gateway.")?;
        decode_json(response).await
    }

    pub async fn download(&self, settings: &PrivateSettings, id: Uuid) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(format!(
                "{}/v1/conversions/{id}/download",
                settings.gateway_url
            ))
            .bearer_auth(&settings.gateway_token)
            .send()
            .await
            .context("Could not download the converted file.")?;
        let response = ensure_success(response).await?;
        Ok(response.bytes().await?.to_vec())
    }
}

async fn decode_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    ensure_success(response)
        .await?
        .json()
        .await
        .context("The gateway returned an invalid response.")
}

async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response
        .json::<ErrorResponse>()
        .await
        .map(|body| body.error)
        .unwrap_or_else(|_| format!("Gateway request failed with {status}."));
    if status == StatusCode::UNAUTHORIZED {
        anyhow::bail!("The gateway rejected the configured token.");
    }
    anyhow::bail!(message)
}
