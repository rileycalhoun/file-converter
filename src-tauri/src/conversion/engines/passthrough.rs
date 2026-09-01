use async_trait::async_trait;

use crate::conversion::{
    ConversionEngine, ConversionError, ConversionRequest, ConversionResult, DetectedFormat,
    EngineOutput, InputFamily,
};

pub struct PdfPassthroughEngine;

#[async_trait]
impl ConversionEngine for PdfPassthroughEngine {
    fn id(&self) -> &'static str {
        "pdf-passthrough"
    }

    fn supports(&self, input: &DetectedFormat) -> bool {
        input.format.family() == InputFamily::Pdf
    }

    async fn convert(&self, request: &ConversionRequest) -> Result<EngineOutput, ConversionError> {
        let source = request.source_path.clone();
        let staged = request.staged_output_path();
        let size = tokio::task::spawn_blocking(move || {
            std::fs::copy(&source, &staged).map_err(|source_error| {
                ConversionError::OutputWriteFailed {
                    path: staged,
                    source: source_error,
                }
            })
        })
        .await
        .map_err(|error| ConversionError::ConversionFailed(error.to_string()))??;
        Ok(EngineOutput::Complete(ConversionResult {
            engine: self.id().into(),
            output_size: size,
        }))
    }
}
