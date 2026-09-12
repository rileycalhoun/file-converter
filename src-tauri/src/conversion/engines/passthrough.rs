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
        Ok(EngineOutput::CopySource(ConversionResult {
            engine: self.id().into(),
            output_size: request.source_size,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn passthrough_defers_its_only_copy_to_destination_publication() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        std::fs::write(&source, b"%PDF-original").unwrap();
        let request = ConversionRequest {
            id: uuid::Uuid::new_v4(),
            source_name: "source.pdf".into(),
            detected: crate::conversion::detect_format(&source).unwrap(),
            source_path: source.clone(),
            output_path: directory.path().join("output.pdf"),
            work_directory: directory.path().join("cache-does-not-exist"),
            source_size: 13,
        };
        assert!(matches!(
            PdfPassthroughEngine.convert(&request).await.unwrap(),
            EngineOutput::CopySource(_)
        ));
        assert!(!request.staged_output_path().exists());
        assert!(!request.output_path.exists());
        assert_eq!(std::fs::read(source).unwrap(), b"%PDF-original");
    }
}
