use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{multipart, Client, StatusCode};
use serde::Deserialize;

use crate::database::PrivateSettings;

pub struct GatewayClient {
    client: Client,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
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

    pub async fn convert_to_pdf(
        &self,
        settings: &PrivateSettings,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let file = multipart::Part::bytes(bytes).file_name(file_name.to_string());
        let form = multipart::Form::new().part("file", file);
        let response = self
            .client
            .post(format!("{}/v1/convert", settings.gateway_url))
            .bearer_auth(&settings.gateway_token)
            .multipart(form)
            .send()
            .await
            .context("Could not reach the conversion gateway.")?;
        let response = ensure_success(response).await?;
        Ok(response
            .bytes()
            .await
            .context("Could not read the converted PDF from the gateway.")?
            .to_vec())
    }
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
