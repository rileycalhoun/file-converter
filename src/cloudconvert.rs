use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct CloudConvertClient {
    client: Client,
    api_key: String,
    api_base: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ConversionState {
    Processing,
    Finished { file_name: String },
    Failed { message: String },
}

pub struct DownloadedFile {
    pub file_name: String,
    pub bytes: Bytes,
}

#[derive(Deserialize)]
struct ApiEnvelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct Job {
    id: Uuid,
    status: String,
    #[serde(default)]
    tasks: Vec<Task>,
}

#[derive(Deserialize)]
struct Task {
    operation: String,
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    result: Option<TaskResult>,
}

#[derive(Deserialize)]
struct TaskResult {
    #[serde(default)]
    files: Vec<ExportedFile>,
}

#[derive(Clone, Deserialize)]
struct ExportedFile {
    filename: String,
    url: String,
}

impl CloudConvertClient {
    pub fn new(api_key: String, api_base: String) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .build()
            .context("failed to construct HTTP client")?;
        Ok(Self {
            client,
            api_key,
            api_base,
        })
    }

    pub async fn create_job(
        &self,
        file_name: &str,
        bytes: &[u8],
        output_format: &str,
    ) -> Result<Uuid> {
        let response = self
            .client
            .post(format!("{}/v2/jobs", self.api_base))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "tasks": {
                    "import-file": {
                        "operation": "import/base64",
                        "file": STANDARD.encode(bytes),
                        "filename": file_name
                    },
                    "convert-file": {
                        "operation": "convert",
                        "input": "import-file",
                        "output_format": output_format
                    },
                    "export-file": {
                        "operation": "export/url",
                        "input": "convert-file"
                    }
                }
            }))
            .send()
            .await
            .context("failed to contact CloudConvert")?
            .error_for_status()
            .context("CloudConvert rejected the conversion job")?;

        Ok(response
            .json::<ApiEnvelope<Job>>()
            .await
            .context("CloudConvert returned an invalid job response")?
            .data
            .id)
    }

    pub async fn job_status(&self, id: Uuid) -> Result<ConversionState> {
        let job = self.get_job(id).await?;
        match job.status.as_str() {
            "finished" => {
                let file = export_file(&job)?;
                Ok(ConversionState::Finished {
                    file_name: file.filename,
                })
            }
            "error" => {
                let message = job
                    .tasks
                    .iter()
                    .find(|task| task.status == "error")
                    .and_then(|task| task.message.clone())
                    .unwrap_or_else(|| "CloudConvert could not complete the conversion".into());
                Ok(ConversionState::Failed { message })
            }
            _ => Ok(ConversionState::Processing),
        }
    }

    pub async fn download(&self, id: Uuid) -> Result<DownloadedFile> {
        let job = self.get_job(id).await?;
        if job.status != "finished" {
            return Err(anyhow!("conversion is not finished"));
        }
        let file = export_file(&job)?;
        let response = self
            .client
            .get(&file.url)
            .send()
            .await
            .context("failed to download converted file")?
            .error_for_status()
            .context("converted file download was rejected")?;
        let bytes = response
            .bytes()
            .await
            .context("failed to read converted file")?;
        Ok(DownloadedFile {
            file_name: file.filename,
            bytes,
        })
    }

    async fn get_job(&self, id: Uuid) -> Result<Job> {
        let response = self
            .client
            .get(format!("{}/v2/jobs/{id}", self.api_base))
            .bearer_auth(&self.api_key)
            .query(&[("include", "tasks")])
            .send()
            .await
            .context("failed to query CloudConvert")?
            .error_for_status()
            .context("CloudConvert rejected the job lookup")?;
        Ok(response
            .json::<ApiEnvelope<Job>>()
            .await
            .context("CloudConvert returned an invalid job response")?
            .data)
    }
}

fn export_file(job: &Job) -> Result<ExportedFile> {
    job.tasks
        .iter()
        .find(|task| task.operation == "export/url")
        .and_then(|task| task.result.as_ref())
        .and_then(|result| result.files.first())
        .cloned()
        .ok_or_else(|| anyhow!("finished job did not contain an exported file"))
}
