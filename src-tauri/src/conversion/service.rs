use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::database::{Conversion, Database, NewConversion};

use super::{
    detect_format,
    engines::{ImagePdfEngine, LibreOfficeWasmEngine, PdfPassthroughEngine},
    ConversionEngine, DetectedFormat, EngineOutput,
};

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("The source file no longer exists: {0}")]
    SourceNotFound(PathBuf),
    #[error("Could not read the source file at {path}: {source}")]
    ReadSource {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("This file format is not supported: {0}")]
    UnsupportedFormat(String),
    #[error("The file format could not be confirmed. {0}")]
    DetectionFailed(String),
    #[error("The document conversion failed. {0}")]
    ConversionFailed(String),
    #[error("The local LibreOffice engine could not be initialized. {0}")]
    WasmInitializationFailed(String),
    #[error(
        "The conversion ran out of memory. Try closing other applications or using a smaller file."
    )]
    OutOfMemory,
    #[error("Could not write the PDF at {path}: {source}")]
    OutputWriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[allow(dead_code)]
    #[error("Conversion was cancelled.")]
    Cancelled,
    #[error("This conversion is no longer active.")]
    StaleJob,
    #[error("The local conversion history could not be updated: {0}")]
    Persistence(String),
}

#[derive(Clone, Debug)]
pub struct ConversionRequest {
    pub id: Uuid,
    pub source_name: String,
    pub source_path: PathBuf,
    pub detected: DetectedFormat,
    pub output_path: PathBuf,
    pub work_directory: PathBuf,
    pub source_size: u64,
}

impl ConversionRequest {
    pub fn staged_output_path(&self) -> PathBuf {
        self.work_directory.join("output.pdf")
    }
}

#[derive(Debug)]
pub struct ConversionResult {
    pub engine: String,
    pub output_size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmConversionTask {
    pub conversion_id: Uuid,
    pub file_name: String,
    pub input_format: String,
    pub output_format: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionStart {
    pub conversion: Conversion,
    pub wasm_task: Option<WasmConversionTask>,
}

pub struct ConversionService {
    database: Arc<Database>,
    work_root: PathBuf,
    engines: Vec<Arc<dyn ConversionEngine>>,
    pending_wasm: Mutex<HashMap<Uuid, ConversionRequest>>,
}

impl ConversionService {
    pub fn new(database: Arc<Database>, work_root: PathBuf) -> Self {
        Self::new_with_engines(
            database,
            work_root,
            vec![
                Arc::new(PdfPassthroughEngine),
                Arc::new(ImagePdfEngine),
                Arc::new(LibreOfficeWasmEngine),
            ],
        )
    }

    fn new_with_engines(
        database: Arc<Database>,
        work_root: PathBuf,
        engines: Vec<Arc<dyn ConversionEngine>>,
    ) -> Self {
        Self {
            database,
            work_root,
            engines,
            pending_wasm: Mutex::new(HashMap::new()),
        }
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn cleanup_stale_workdirs(&self) -> Result<(), ConversionError> {
        if !self.work_root.exists() {
            std::fs::create_dir_all(&self.work_root).map_err(|source| {
                ConversionError::OutputWriteFailed {
                    path: self.work_root.clone(),
                    source,
                }
            })?;
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.work_root).map_err(|source| {
            ConversionError::OutputWriteFailed {
                path: self.work_root.clone(),
                source,
            }
        })? {
            let entry = entry.map_err(|source| ConversionError::OutputWriteFailed {
                path: self.work_root.clone(),
                source,
            })?;
            if entry.path().is_dir() {
                std::fs::remove_dir_all(entry.path()).map_err(|source| {
                    ConversionError::OutputWriteFailed {
                        path: self.work_root.clone(),
                        source,
                    }
                })?;
            }
        }
        Ok(())
    }

    pub async fn start(&self, source_path: PathBuf) -> Result<ConversionStart, ConversionError> {
        let source_name = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                ConversionError::DetectionFailed("The selected file has no valid name.".into())
            })?
            .to_string();
        let metadata = std::fs::metadata(&source_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ConversionError::SourceNotFound(source_path.clone())
            } else {
                ConversionError::ReadSource {
                    path: source_path.clone(),
                    source,
                }
            }
        })?;
        if !metadata.is_file() {
            return Err(ConversionError::SourceNotFound(source_path));
        }
        let detected = detect_format(&source_path)?;
        let engine = self
            .engines
            .iter()
            .find(|engine| engine.supports(&detected))
            .cloned()
            .ok_or_else(|| {
                ConversionError::UnsupportedFormat(detected.format.extension().into())
            })?;
        if engine.id() == "libreoffice-wasm"
            && !self
                .pending_wasm
                .lock()
                .map_err(|_| {
                    ConversionError::ConversionFailed("conversion queue lock was poisoned".into())
                })?
                .is_empty()
        {
            return Err(ConversionError::ConversionFailed(
                "Another LibreOffice conversion is already running. Wait for it to finish or cancel it."
                    .into(),
            ));
        }

        let id = Uuid::new_v4();
        let output_name = pdf_file_name(&source_name);
        let output_directory = source_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| {
                ConversionError::DetectionFailed(
                    "The selected file does not have a usable parent folder.".into(),
                )
            })?;
        let output_path = unique_output_path(output_directory, id, &output_name);
        let work_directory = self.work_root.join(id.to_string());
        std::fs::create_dir_all(&work_directory).map_err(|source| {
            ConversionError::OutputWriteFailed {
                path: work_directory.clone(),
                source,
            }
        })?;
        let request = ConversionRequest {
            id,
            source_name,
            source_path,
            detected,
            output_path,
            work_directory,
            source_size: metadata.len(),
        };
        let conversion = self
            .database
            .insert_conversion(NewConversion {
                id,
                source_name: &request.source_name,
                source_path: &request.source_path,
                detected_format: request.detected.format.extension(),
                output_format: "pdf",
                engine: engine.id(),
                source_size: request.source_size,
            })
            .map_err(|error| ConversionError::Persistence(error.to_string()))?;

        match engine.convert(&request).await {
            Ok(EngineOutput::Complete(result)) => {
                let conversion = self.finish_request(&request, result)?;
                Ok(ConversionStart {
                    conversion,
                    wasm_task: None,
                })
            }
            Ok(EngineOutput::RequiresBrowser(task)) => {
                self.pending_wasm
                    .lock()
                    .map_err(|_| {
                        ConversionError::ConversionFailed(
                            "conversion queue lock was poisoned".into(),
                        )
                    })?
                    .insert(id, request);
                Ok(ConversionStart {
                    conversion,
                    wasm_task: Some(task),
                })
            }
            Err(error) => {
                self.cleanup_request(&request);
                let failed = self.database.mark_failed(id, &error.to_string()).map_err(
                    |database_error| ConversionError::Persistence(database_error.to_string()),
                )?;
                Ok(ConversionStart {
                    conversion: failed,
                    wasm_task: None,
                })
            }
        }
    }

    pub async fn reconvert(&self, id: Uuid) -> Result<ConversionStart, ConversionError> {
        let conversion = self
            .database
            .get_conversion(id)
            .map_err(|error| ConversionError::Persistence(error.to_string()))?;
        let source_path = conversion.source_path.map(PathBuf::from).ok_or_else(|| {
            ConversionError::SourceNotFound(PathBuf::from(conversion.source_name))
        })?;
        self.start(source_path).await
    }

    pub async fn read_wasm_input(&self, id: Uuid) -> Result<Vec<u8>, ConversionError> {
        let source_path = self
            .pending_wasm
            .lock()
            .map_err(|_| {
                ConversionError::ConversionFailed("conversion queue lock was poisoned".into())
            })?
            .get(&id)
            .map(|request| request.source_path.clone())
            .ok_or(ConversionError::StaleJob)?;
        tokio::fs::read(&source_path)
            .await
            .map_err(|source| ConversionError::ReadSource {
                path: source_path,
                source,
            })
    }

    pub fn complete_wasm(&self, id: Uuid, pdf: &[u8]) -> Result<Conversion, ConversionError> {
        if !pdf.starts_with(b"%PDF-") {
            return self.fail_wasm(id, "The conversion engine returned an invalid PDF.");
        }
        let request = self.take_pending(id)?;
        let staged = request.staged_output_path();
        if let Err(source) = std::fs::write(&staged, pdf) {
            let error = ConversionError::OutputWriteFailed {
                path: staged,
                source,
            };
            self.cleanup_request(&request);
            let _ = self.database.mark_failed(id, &error.to_string());
            return Err(error);
        }
        self.finish_request(
            &request,
            ConversionResult {
                engine: "libreoffice-wasm".into(),
                output_size: pdf.len() as u64,
            },
        )
    }

    pub fn fail_wasm(&self, id: Uuid, error: &str) -> Result<Conversion, ConversionError> {
        let request = self.take_pending(id)?;
        self.cleanup_request(&request);
        self.database
            .mark_failed(id, &friendly_wasm_error(error))
            .map_err(|database_error| ConversionError::Persistence(database_error.to_string()))
    }

    pub fn cancel(&self, id: Uuid) -> Result<Conversion, ConversionError> {
        let request = self.take_pending(id)?;
        self.cleanup_request(&request);
        self.database
            .mark_cancelled(id)
            .map_err(|error| ConversionError::Persistence(error.to_string()))
    }

    pub fn delete_history(&self, id: Uuid) -> Result<Option<String>, ConversionError> {
        if self
            .pending_wasm
            .lock()
            .map_err(|_| {
                ConversionError::ConversionFailed("conversion queue lock was poisoned".into())
            })?
            .contains_key(&id)
        {
            return Err(ConversionError::ConversionFailed(
                "Active conversions cannot be deleted. Cancel or finish the conversion first."
                    .into(),
            ));
        }
        self.database
            .delete_conversion(id)
            .map_err(|error| ConversionError::Persistence(error.to_string()))
    }

    fn take_pending(&self, id: Uuid) -> Result<ConversionRequest, ConversionError> {
        self.pending_wasm
            .lock()
            .map_err(|_| {
                ConversionError::ConversionFailed("conversion queue lock was poisoned".into())
            })?
            .remove(&id)
            .ok_or(ConversionError::StaleJob)
    }

    fn finish_request(
        &self,
        request: &ConversionRequest,
        result: ConversionResult,
    ) -> Result<Conversion, ConversionError> {
        let staged = request.staged_output_path();
        if let Err(source) = std::fs::copy(&staged, &request.output_path) {
            let error = ConversionError::OutputWriteFailed {
                path: request.output_path.clone(),
                source,
            };
            let _ = std::fs::remove_file(&request.output_path);
            self.cleanup_request(request);
            let _ = self.database.mark_failed(request.id, &error.to_string());
            return Err(error);
        }
        let conversion = match self.database.mark_finished(
            request.id,
            &request.output_path,
            &result.engine,
            result.output_size,
        ) {
            Ok(conversion) => conversion,
            Err(error) => {
                let _ = std::fs::remove_file(&request.output_path);
                self.cleanup_request(request);
                return Err(ConversionError::Persistence(error.to_string()));
            }
        };
        self.cleanup_request(request);
        Ok(conversion)
    }

    fn cleanup_request(&self, request: &ConversionRequest) {
        let _ = std::fs::remove_dir_all(&request.work_directory);
    }
}

fn friendly_wasm_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("out of memory") || lower.contains("memory access out of bounds") {
        return ConversionError::OutOfMemory.to_string();
    }
    if lower.contains("initial") || lower.contains("wasm") || lower.contains("worker") {
        return ConversionError::WasmInitializationFailed(error.to_string()).to_string();
    }
    ConversionError::ConversionFailed(error.to_string()).to_string()
}

pub fn pdf_file_name(value: &str) -> String {
    let stem = Path::new(value)
        .file_name()
        .and_then(|name| Path::new(name).file_stem())
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("converted-file");
    let safe: String = stem
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\' | ':' | '"'))
        .collect();
    format!(
        "{}.pdf",
        if safe.trim().is_empty() {
            "converted-file"
        } else {
            safe.trim()
        }
    )
}

pub fn unique_output_path(directory: &Path, id: Uuid, file_name: &str) -> PathBuf {
    directory.join(format!("{}-{file_name}", &id.to_string()[..8]))
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::{pdf_file_name, ConversionService};
    use crate::database::Database;

    #[test]
    fn output_names_are_safe_pdf_files() {
        assert_eq!(pdf_file_name("letter.docx"), "letter.pdf");
        assert_eq!(pdf_file_name("../../slides.pptx"), "slides.pdf");
        assert_eq!(pdf_file_name("bad:name.docx"), "badname.pdf");
    }

    #[tokio::test]
    async fn existing_pdf_is_copied_and_recorded() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("already.pdf");
        std::fs::write(&source, b"%PDF-1.7\nfixture").unwrap();
        let started = service.start(source.clone()).await.unwrap();
        assert_eq!(started.conversion.status, "finished");
        assert_eq!(
            started.conversion.engine.as_deref(),
            Some("pdf-passthrough")
        );
        let output_path = PathBuf::from(started.conversion.output_path.unwrap());
        assert_eq!(output_path.parent(), source.parent());
        assert!(std::fs::read(output_path).unwrap().starts_with(b"%PDF-"));
    }

    #[tokio::test]
    async fn reconvert_fails_when_the_original_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("already.pdf");
        std::fs::write(&source, b"%PDF-1.7\nfixture").unwrap();
        let id = service.start(source.clone()).await.unwrap().conversion.id;
        std::fs::remove_file(source).unwrap();
        assert!(service.reconvert(id).await.is_err());
    }

    #[tokio::test]
    async fn office_routes_to_wasm_and_failures_are_recorded() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"local fixture").unwrap();
        let started = service.start(source).await.unwrap();
        assert_eq!(
            started.conversion.engine.as_deref(),
            Some("libreoffice-wasm")
        );
        let task = started.wasm_task.unwrap();
        assert_eq!(task.input_format, "txt");
        let failed = service
            .fail_wasm(task.conversion_id, "LOAD_FAILED: corrupt document")
            .unwrap();
        assert_eq!(failed.status, "failed");
        assert!(failed.error.unwrap().contains("conversion failed"));
    }

    #[tokio::test]
    async fn cancellation_rejects_stale_wasm_output() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"local fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();
        let cancelled = service.cancel(task.conversion_id).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(service
            .complete_wasm(task.conversion_id, b"%PDF-1.7\nstale")
            .is_err());
    }

    #[tokio::test]
    async fn active_wasm_conversion_cannot_be_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"local fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();

        assert!(service.delete_history(task.conversion_id).is_err());
        assert_eq!(
            service
                .database()
                .get_conversion(task.conversion_id)
                .unwrap()
                .status,
            "processing"
        );

        service.cancel(task.conversion_id).unwrap();
        assert!(service.delete_history(task.conversion_id).is_ok());
        assert!(service
            .database()
            .get_conversion(task.conversion_id)
            .is_err());
    }

    #[tokio::test]
    async fn missing_pdf_can_be_reconverted_when_source_exists() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("already.pdf");
        std::fs::write(&source, b"%PDF-1.7\nfixture").unwrap();
        let first = service.start(source).await.unwrap().conversion;
        std::fs::remove_file(first.output_path.as_ref().unwrap()).unwrap();
        let second = service.reconvert(first.id).await.unwrap().conversion;
        assert_eq!(second.status, "finished");
        assert_ne!(second.id, first.id);
        assert!(std::path::Path::new(second.output_path.as_ref().unwrap()).is_file());
    }
}
