use async_trait::async_trait;

use super::{
    ConversionError, ConversionRequest, ConversionResult, DetectedFormat, WasmConversionTask,
};

#[derive(Debug)]
pub enum EngineOutput {
    Complete(ConversionResult),
    RequiresBrowser(WasmConversionTask),
}

#[async_trait]
pub trait ConversionEngine: Send + Sync {
    fn id(&self) -> &'static str;
    fn supports(&self, input: &DetectedFormat) -> bool;
    async fn convert(&self, request: &ConversionRequest) -> Result<EngineOutput, ConversionError>;
}
