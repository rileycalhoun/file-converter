use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{ipc::InvokeBody, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::{
    conversion::{
        detect_format, publish_output, supported_formats, unique_output_path, ConversionStart,
        SupportedFormat,
    },
    database::{Conversion, Database},
    AppState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DetectedSource {
    format: String,
    family: crate::conversion::InputFamily,
    engine: String,
    source_size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenConversionResult {
    missing: bool,
    restorable: bool,
    reconvertible: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryEntry {
    #[serde(flatten)]
    conversion: Conversion,
    #[serde(flatten)]
    availability: OpenConversionResult,
}

#[tauri::command]
pub(crate) fn get_supported_formats() -> Vec<SupportedFormat> {
    supported_formats().to_vec()
}

#[tauri::command]
pub(crate) fn inspect_source(input_path: String) -> Result<DetectedSource, String> {
    let path = PathBuf::from(input_path);
    let detected = detect_format(&path).map_err(error_message)?;
    let source_size = std::fs::metadata(&path).map_err(error_message)?.len();
    let engine = supported_formats()
        .iter()
        .find(|entry| entry.format == detected.format.extension())
        .map(|entry| entry.engine)
        .ok_or_else(|| "This detected format does not have a conversion engine.".to_string())?;
    Ok(DetectedSource {
        format: detected.format.extension().into(),
        family: detected.format.family(),
        engine: engine.into(),
        source_size,
    })
}

#[tauri::command]
pub(crate) fn list_conversions(state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    let database = state.service.database();
    database
        .list_conversions()
        .map_err(error_message)?
        .into_iter()
        .map(|conversion| history_entry(database, conversion))
        .collect()
}

#[tauri::command]
pub(crate) async fn start_conversion(
    state: State<'_, AppState>,
    input_path: String,
) -> Result<ConversionStart, String> {
    state
        .service
        .start(PathBuf::from(input_path))
        .await
        .map_err(error_message)
}

#[tauri::command]
pub(crate) async fn reconvert(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<ConversionStart, String> {
    state.service.reconvert(id).await.map_err(error_message)
}

#[tauri::command]
pub(crate) async fn read_conversion_input(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<tauri::ipc::Response, String> {
    state
        .service
        .read_wasm_input(id)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(error_message)
}

#[tauri::command]
pub(crate) fn complete_wasm_conversion(
    request: tauri::ipc::Request<'_>,
    state: State<'_, AppState>,
) -> Result<Conversion, String> {
    let id = conversion_id_header(&request)?;
    let InvokeBody::Raw(pdf) = request.body() else {
        return Err("The converted PDF must be sent as a binary payload.".into());
    };
    state.service.complete_wasm(id, pdf).map_err(error_message)
}

#[tauri::command]
pub(crate) fn fail_wasm_conversion(
    state: State<'_, AppState>,
    id: Uuid,
    error: String,
) -> Result<Conversion, String> {
    state.service.fail_wasm(id, &error).map_err(error_message)
}

#[tauri::command]
pub(crate) fn cancel_conversion(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<Conversion, String> {
    state.service.cancel(id).map_err(error_message)
}

#[tauri::command]
pub(crate) fn open_conversion(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<OpenConversionResult, String> {
    let conversion = state
        .service
        .database()
        .get_conversion(id)
        .map_err(error_message)?;
    let availability = conversion_availability(state.service.database(), &conversion)?;
    if availability.missing {
        return Ok(availability);
    }
    let path = conversion
        .output_path
        .as_deref()
        .ok_or_else(|| "This conversion does not have an output file.".to_string())?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(error_message)?;
    Ok(availability)
}

#[tauri::command]
pub(crate) fn restore_conversion(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<(), String> {
    let output_path =
        restore_stored_conversion(state.service.database(), &state.application_home, id)?;
    app.opener()
        .open_path(output_path.to_string_lossy(), None::<&str>)
        .map_err(error_message)
}

fn restore_stored_conversion(
    database: &Database,
    application_home: &Path,
    id: Uuid,
) -> Result<PathBuf, String> {
    let conversion = database.get_conversion(id).map_err(error_message)?;
    let stored = database.stored_file(id).map_err(error_message)?;
    let output_name = crate::conversion::pdf_file_name(&stored.source_name);
    let output_directory = conversion
        .source_path
        .as_deref()
        .and_then(|source| Path::new(source).parent())
        .filter(|directory| directory.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| application_home.join("legacy-restored-files"));
    let output_path = unique_output_path(&output_directory, id, &output_name);
    std::fs::create_dir_all(&output_directory)
        .map_err(|error| format!("Could not prepare the output folder: {error}"))?;
    let published = publish_output(&output_path, stored.bytes.as_slice())
        .map_err(|error| format!("Could not recreate the converted file: {error}"))?;
    database
        .update_output_path(id, published.path())
        .map_err(error_message)?;
    Ok(published.commit())
}

#[tauri::command]
pub(crate) fn delete_conversion(state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    if let Some(path) = state.service.delete_history(id).map_err(error_message)? {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Removed the history entry but not its file: {error}"
                ))
            }
        }
    }
    Ok(())
}

fn conversion_id_header(request: &tauri::ipc::Request<'_>) -> Result<Uuid, String> {
    let value = request
        .headers()
        .get("x-conversion-id")
        .ok_or_else(|| "The conversion ID header is missing.".to_string())?
        .to_str()
        .map_err(|_| "The conversion ID header is invalid.".to_string())?;
    Uuid::parse_str(value).map_err(|_| "The conversion ID is invalid.".to_string())
}

fn history_entry(database: &Database, conversion: Conversion) -> Result<HistoryEntry, String> {
    let availability = conversion_availability(database, &conversion)?;
    Ok(HistoryEntry {
        conversion,
        availability,
    })
}

fn conversion_availability(
    database: &Database,
    conversion: &Conversion,
) -> Result<OpenConversionResult, String> {
    let missing = conversion
        .output_path
        .as_deref()
        .map(Path::new)
        .is_none_or(|path| !path.is_file());
    Ok(OpenConversionResult {
        missing,
        restorable: missing
            && database
                .has_stored_file(conversion.id)
                .map_err(error_message)?,
        reconvertible: missing
            && conversion
                .source_path
                .as_deref()
                .is_some_and(|source| Path::new(source).is_file()),
    })
}

fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::NewConversion;

    fn stored_conversion(directory: &Path) -> (Database, Uuid, PathBuf) {
        let db_path = directory.join("history.sqlite3");
        let database = Database::open(&db_path).unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(NewConversion {
                id,
                source_name: "source.docx",
                source_path: &directory.join("source.docx"),
                detected_format: "docx",
                output_format: "pdf",
                engine: "legacy",
                source_size: 6,
            })
            .unwrap();
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE conversions SET status = 'finished', output_data = ?2 WHERE id = ?1",
                rusqlite::params![id.to_string(), b"%PDF-restored".as_slice()],
            )
            .unwrap();
        (database, id, db_path)
    }

    #[test]
    fn repeated_restoration_preserves_existing_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let (database, id, _) = stored_conversion(directory.path());
        let preferred = unique_output_path(directory.path(), id, "source.pdf");
        std::fs::write(&preferred, b"unrelated").unwrap();
        let first = restore_stored_conversion(&database, directory.path(), id).unwrap();
        let second = restore_stored_conversion(&database, directory.path(), id).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&preferred).unwrap(), b"unrelated");
        assert_eq!(std::fs::read(&first).unwrap(), b"%PDF-restored");
        assert_eq!(std::fs::read(&second).unwrap(), b"%PDF-restored");
        assert_eq!(
            database.get_conversion(id).unwrap().output_path.unwrap(),
            second.to_string_lossy()
        );
    }

    #[test]
    fn concurrent_restoration_never_clobbers_another_restore() {
        let directory = tempfile::tempdir().unwrap();
        let (database, id, _) = stored_conversion(directory.path());
        let database = std::sync::Arc::new(database);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let database = database.clone();
                let barrier = barrier.clone();
                let home = directory.path().to_path_buf();
                std::thread::spawn(move || {
                    barrier.wait();
                    restore_stored_conversion(&database, &home, id).unwrap()
                })
            })
            .collect();
        let paths: std::collections::HashSet<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(paths.len(), 4);
        for path in paths {
            assert_eq!(std::fs::read(path).unwrap(), b"%PDF-restored");
        }
    }

    #[test]
    fn restore_persistence_failure_removes_only_the_new_file() {
        let directory = tempfile::tempdir().unwrap();
        let (database, id, db_path) = stored_conversion(directory.path());
        let preferred = unique_output_path(directory.path(), id, "source.pdf");
        std::fs::write(&preferred, b"unrelated").unwrap();
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_restore BEFORE UPDATE OF output_path ON conversions
             BEGIN SELECT RAISE(FAIL, 'injected persistence failure'); END;",
            )
            .unwrap();
        assert!(restore_stored_conversion(&database, directory.path(), id).is_err());
        assert_eq!(std::fs::read(preferred).unwrap(), b"unrelated");
        assert!(database.get_conversion(id).unwrap().output_path.is_none());
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

    #[test]
    fn history_availability_tracks_missing_outputs_and_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source.docx");
        let output_path = directory.path().join("source.pdf");
        std::fs::write(&source_path, b"source").unwrap();
        std::fs::write(&output_path, b"%PDF-output").unwrap();

        let database = Database::open(&directory.path().join("history.sqlite3")).unwrap();
        let id = Uuid::new_v4();
        database
            .insert_conversion(NewConversion {
                id,
                source_name: "source.docx",
                source_path: &source_path,
                detected_format: "docx",
                output_format: "pdf",
                engine: "libreoffice-wasm",
                source_size: 6,
            })
            .unwrap();
        let conversion = database
            .mark_finished(id, &output_path, "libreoffice-wasm", 11)
            .unwrap();

        let available = conversion_availability(&database, &conversion).unwrap();
        assert!(!available.missing);
        assert!(!available.restorable);
        assert!(!available.reconvertible);

        std::fs::remove_file(&output_path).unwrap();
        let missing = conversion_availability(&database, &conversion).unwrap();
        assert!(missing.missing);
        assert!(!missing.restorable);
        assert!(missing.reconvertible);

        std::fs::remove_file(&source_path).unwrap();
        let unavailable = conversion_availability(&database, &conversion).unwrap();
        assert!(unavailable.missing);
        assert!(!unavailable.reconvertible);
    }
}
