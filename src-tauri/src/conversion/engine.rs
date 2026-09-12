use async_trait::async_trait;

use super::{
    ConversionError, ConversionRequest, ConversionResult, DetectedFormat, WasmConversionTask,
};

#[derive(Debug)]
pub enum EngineOutput {
    Complete(ConversionResult),
    /// The source is already a PDF. Publish an independent destination copy
    /// directly, without writing another complete PDF into the cache first.
    CopySource(ConversionResult),
    RequiresBrowser(WasmConversionTask),
}

#[async_trait]
pub trait ConversionEngine: Send + Sync {
    fn id(&self) -> &'static str;
    fn supports(&self, input: &DetectedFormat) -> bool;
    async fn convert(&self, request: &ConversionRequest) -> Result<EngineOutput, ConversionError>;
}
