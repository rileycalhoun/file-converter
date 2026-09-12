mod blocking_io;
mod detector;
mod engine;
mod output;
mod registry;
mod service;

pub mod engines;

pub use detector::detect_format;
pub use engine::{ConversionEngine, EngineOutput};
pub(crate) use output::publish_output;
pub use registry::{supported_formats, DetectedFormat, InputFamily, SupportedFormat};
pub use service::{
    pdf_file_name, unique_output_path, ConversionError, ConversionRequest, ConversionResult,
    ConversionService, ConversionStart, WasmConversionTask,
};
