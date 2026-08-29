use std::time::Duration;

use anyhow::{Context, Result};
use axum::body::Bytes;
use reqwest::{multipart, Client};

#[derive(Clone)]
pub struct GotenbergClient {
    client: Client,
    base_url: String,
}

impl GotenbergClient {
    pub fn new(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .context("failed to construct the Gotenberg HTTP client")?;
        Ok(Self { client, base_url })
    }

    pub async fn convert_to_pdf(&self, file_name: &str, bytes: Bytes) -> Result<Bytes> {
        let file = multipart::Part::bytes(bytes.to_vec()).file_name(file_name.to_string());
        let form = multipart::Form::new()
            .part("files", file)
            .text("updateIndexes", "false");
        let response = self
            .client
            .post(format!("{}/forms/libreoffice/convert", self.base_url))
            .header("Gotenberg-Output-Filename", output_file_name(file_name))
            .multipart(form)
            .send()
            .await
            .context("failed to contact Gotenberg")?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.trim();
            if detail.is_empty() {
                anyhow::bail!("Gotenberg rejected the conversion with {status}");
            }
            anyhow::bail!("Gotenberg rejected the conversion with {status}: {detail}");
        }

        response
            .bytes()
            .await
            .context("failed to read the PDF returned by Gotenberg")
    }
}

pub fn output_file_name(input: &str) -> String {
    let stem = std::path::Path::new(input)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("converted");
    let safe_stem: String = stem
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '"' | '\\'))
        .collect();
    let safe_stem = safe_stem.trim();
    format!(
        "{}.pdf",
        if safe_stem.is_empty() {
            "converted"
        } else {
            safe_stem
        }
    )
}

#[cfg(test)]
mod tests {
    use super::output_file_name;

    #[test]
    fn derives_a_safe_pdf_name() {
        assert_eq!(output_file_name("report.docx"), "report.pdf");
        assert_eq!(output_file_name("../../quarterly.pptx"), "quarterly.pdf");
        assert_eq!(output_file_name("bad\"name.docx"), "badname.pdf");
    }
}
