use async_trait::async_trait;

use crate::conversion::{
    ConversionEngine, ConversionError, ConversionRequest, DetectedFormat, EngineOutput,
    InputFamily, WasmConversionTask,
};

pub struct LibreOfficeWasmEngine;

#[async_trait]
impl ConversionEngine for LibreOfficeWasmEngine {
    fn id(&self) -> &'static str {
        "libreoffice-wasm"
    }

    fn supports(&self, input: &DetectedFormat) -> bool {
        matches!(
            input.format.family(),
            InputFamily::Document
                | InputFamily::Spreadsheet
                | InputFamily::Presentation
                | InputFamily::Drawing
                | InputFamily::Text
        )
    }

    async fn convert(&self, request: &ConversionRequest) -> Result<EngineOutput, ConversionError> {
        Ok(EngineOutput::RequiresBrowser(WasmConversionTask {
            conversion_id: request.id,
            file_name: request.source_name.clone(),
            input_format: request.detected.format.extension().to_string(),
            output_format: "pdf".into(),
        }))
    }
}
