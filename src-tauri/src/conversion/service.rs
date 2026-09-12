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
    blocking_io::BlockingIo,
    detect_format,
    engines::{ImagePdfEngine, LibreOfficeWasmEngine, PdfPassthroughEngine},
    output::{check_output_access, publish_source_output, publish_staged_output, PublishedOutput},
    publish_output, ConversionEngine, DetectedFormat, EngineOutput,
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
    #[error(
        "Could not write the PDF at {path}: {source}{}",
        output_write_guidance(source)
    )]
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

fn output_write_guidance(error: &std::io::Error) -> &'static str {
    if error.kind() != std::io::ErrorKind::PermissionDenied {
        return "";
    }
    if cfg!(target_os = "macos") {
        "\nCheck this folder's Sharing & Permissions in Finder > Get Info and confirm your account has Read & Write access. Also check System Settings > Privacy & Security > Files and Folders > File Converter and enable access to the source folder if listed, then quit and reopen the app. You can also move the source document to a folder you can write to and retry."
    } else {
        "\nCheck that your account has write access to this folder, or move the source document to a folder you can write to and retry."
    }
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
    blocking_io: BlockingIo,
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
            blocking_io: BlockingIo::new(),
        }
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn database_handle(&self) -> Arc<Database> {
        self.database.clone()
    }

    pub(crate) fn blocking_io(&self) -> BlockingIo {
        self.blocking_io.clone()
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
        let access_path = output_path.clone();
        self.blocking_io
            .run(move || {
                check_output_access(&access_path).map_err(|source| {
                    ConversionError::OutputWriteFailed {
                        path: access_path,
                        source,
                    }
                })
            })
            .await
            .map_err(|error| ConversionError::ConversionFailed(error.to_string()))??;
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
            Ok(EngineOutput::Complete(result)) => self.finish_native(request, result, false).await,
            Ok(EngineOutput::CopySource(result)) => self.finish_native(request, result, true).await,
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
                Self::cleanup_request(&request);
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

    async fn finish_native(
        &self,
        request: ConversionRequest,
        mut result: ConversionResult,
        copy_source: bool,
    ) -> Result<ConversionStart, ConversionError> {
        let database = self.database.clone();
        let conversion = self
            .blocking_io
            .run(move || {
                let published = if copy_source {
                    // Never hard-link the user's original: subsequent edits to a
                    // source or a converted PDF must not modify the other file.
                    publish_source_output(&request.output_path, &request.source_path)
                } else {
                    publish_staged_output(&request.output_path, &request.staged_output_path())
                };
                if let Ok(output) = &published {
                    result.output_size = output.size();
                }
                Self::finish_request(&database, &request, result, published)
            })
            .await??;
        Ok(ConversionStart {
            conversion,
            wasm_task: None,
        })
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

    pub async fn complete_wasm(&self, id: Uuid, pdf: &[u8]) -> Result<Conversion, ConversionError> {
        // Acquire capacity before cloning a potentially large IPC payload or
        // claiming the pending request. Queued jobs remain cancellable.
        let permit = self.blocking_io.acquire().await?;
        let request = self.take_pending(id)?;
        let valid = pdf.starts_with(b"%PDF-");
        let pdf = if valid { pdf.to_vec() } else { Vec::new() };
        let database = self.database.clone();
        BlockingIo::run_with_permit(permit, move || {
            if !valid {
                Self::cleanup_request(&request);
                return database
                    .mark_failed(id, "The conversion engine returned an invalid PDF.")
                    .map_err(|error| ConversionError::Persistence(error.to_string()));
            }
            let result = ConversionResult {
                engine: "libreoffice-wasm".into(),
                output_size: pdf.len() as u64,
            };
            // Browser output can go straight to destination-local staging. It
            // does not need a full write and reread in the application cache.
            let published = publish_output(&request.output_path, pdf.as_slice());
            Self::finish_request(&database, &request, result, published)
        })
        .await?
    }

    pub fn fail_wasm(&self, id: Uuid, error: &str) -> Result<Conversion, ConversionError> {
        let request = self.take_pending(id)?;
        Self::cleanup_request(&request);
        self.database
            .mark_failed(id, &friendly_wasm_error(error))
            .map_err(|database_error| ConversionError::Persistence(database_error.to_string()))
    }

    pub fn cancel(&self, id: Uuid) -> Result<Conversion, ConversionError> {
        let request = self.take_pending(id)?;
        Self::cleanup_request(&request);
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
        database: &Database,
        request: &ConversionRequest,
        result: ConversionResult,
        published: std::io::Result<PublishedOutput>,
    ) -> Result<Conversion, ConversionError> {
        let published = match published {
            Ok(published) => published,
            Err(source) => {
                let error = ConversionError::OutputWriteFailed {
                    path: request.output_path.clone(),
                    source,
                };
                Self::cleanup_request(request);
                let _ = database.mark_failed(request.id, &error.to_string());
                return Err(error);
            }
        };
        let conversion = match database.mark_finished(
            request.id,
            published.path(),
            &result.engine,
            result.output_size,
        ) {
            Ok(conversion) => conversion,
            Err(error) => {
                Self::cleanup_request(request);
                return Err(ConversionError::Persistence(error.to_string()));
            }
        };
        published.commit();
        Self::cleanup_request(request);
        Ok(conversion)
    }

    fn cleanup_request(request: &ConversionRequest) {
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
    directory.join(format!("{id}-{file_name}"))
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

    #[test]
    fn output_candidates_use_the_entire_conversion_id() {
        let first = uuid::Uuid::parse_str("12345678-0000-4000-8000-000000000001").unwrap();
        let second = uuid::Uuid::parse_str("12345678-0000-4000-8000-000000000002").unwrap();
        assert_ne!(
            super::unique_output_path(std::path::Path::new("/output"), first, "report.pdf"),
            super::unique_output_path(std::path::Path::new("/output"), second, "report.pdf"),
        );
    }

    #[tokio::test]
    async fn wasm_completion_retries_a_name_created_while_conversion_was_running() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();
        let preferred = service.pending_wasm.lock().unwrap()[&task.conversion_id]
            .output_path
            .clone();
        std::fs::write(&preferred, b"created by someone else").unwrap();

        let finished = service
            .complete_wasm(task.conversion_id, b"%PDF-completed")
            .await
            .unwrap();
        let actual = PathBuf::from(finished.output_path.unwrap());
        assert_ne!(actual, preferred);
        assert_eq!(std::fs::read(actual).unwrap(), b"%PDF-completed");
        assert_eq!(
            std::fs::read(preferred).unwrap(),
            b"created by someone else"
        );
    }

    #[tokio::test]
    async fn failed_completion_persistence_rolls_back_only_its_publication() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("db.sqlite3");
        let database = Arc::new(Database::open(&db_path).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();
        let request = service.pending_wasm.lock().unwrap()[&task.conversion_id].clone();
        std::fs::write(&request.output_path, b"existing PDF").unwrap();
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_completion BEFORE UPDATE OF output_path ON conversions
             BEGIN SELECT RAISE(FAIL, 'injected persistence failure'); END;",
            )
            .unwrap();

        assert!(service
            .complete_wasm(task.conversion_id, b"%PDF-completed")
            .await
            .is_err());
        assert_eq!(std::fs::read(request.output_path).unwrap(), b"existing PDF");
        assert!(!request.work_directory.exists());
        assert_eq!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "pdf"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn browser_pdf_is_published_without_a_second_cache_copy() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();
        let work_directory = service.pending_wasm.lock().unwrap()[&task.conversion_id]
            .work_directory
            .clone();
        // Browser completion needs only destination-local staging, even if its
        // original cache directory is no longer writable or available.
        std::fs::remove_dir_all(&work_directory).unwrap();
        let mut pdf = vec![b' '; 2 * 1024 * 1024];
        pdf[..5].copy_from_slice(b"%PDF-");
        let finished = service
            .complete_wasm(task.conversion_id, &pdf)
            .await
            .unwrap();
        assert_eq!(std::fs::read(finished.output_path.unwrap()).unwrap(), pdf);
        assert!(!work_directory.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_waiting_for_worker_capacity_remains_cancellable() {
        use std::{future::Future, task::Poll};

        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&directory.path().join("db.sqlite3")).unwrap());
        let service = ConversionService::new(database, directory.path().join("work"));
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"fixture").unwrap();
        let task = service.start(source).await.unwrap().wasm_task.unwrap();
        let first = service.blocking_io.acquire().await.unwrap();
        let second = service.blocking_io.acquire().await.unwrap();
        let completion = service.complete_wasm(task.conversion_id, b"%PDF-completed");
        tokio::pin!(completion);
        std::future::poll_fn(|context| {
            assert!(completion.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(
            service.cancel(task.conversion_id).unwrap().status,
            "cancelled"
        );
        drop((first, second));
        assert!(matches!(
            completion.await,
            Err(super::ConversionError::StaleJob)
        ));
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
        assert_eq!(started.conversion.output_size, Some(16));
        assert_eq!(std::fs::read(&output_path).unwrap(), b"%PDF-1.7\nfixture");
        assert!(!same_file::is_same_file(&source, &output_path).unwrap());
        std::fs::write(&source, b"edited original").unwrap();
        assert_eq!(std::fs::read(&output_path).unwrap(), b"%PDF-1.7\nfixture");
        std::fs::write(&output_path, b"edited conversion").unwrap();
        assert_eq!(std::fs::read(source).unwrap(), b"edited original");
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
            .await
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
