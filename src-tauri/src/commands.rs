use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{ipc::InvokeBody, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::{
    conversion::{
        detect_format, supported_formats, unique_output_path, ConversionStart, SupportedFormat,
    },
    database::Conversion,
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
pub(crate) fn list_conversions(state: State<'_, AppState>) -> Result<Vec<Conversion>, String> {
    state
        .service
        .database()
        .list_conversions()
        .map_err(error_message)
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
    let path = conversion
        .output_path
        .as_deref()
        .ok_or_else(|| "This conversion does not have an output file.".to_string())?;
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            app.opener()
                .open_path(path, None::<&str>)
                .map_err(error_message)?;
            Ok(OpenConversionResult {
                missing: false,
                restorable: false,
                reconvertible: false,
            })
        }
        Ok(_) => Err("The saved output path is not a file.".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(OpenConversionResult {
            missing: true,
            restorable: state
                .service
                .database()
                .has_stored_file(id)
                .map_err(error_message)?,
            reconvertible: conversion
                .source_path
                .as_deref()
                .is_some_and(|source| Path::new(source).is_file()),
        }),
        Err(error) => Err(format!("Could not check the converted file: {error}")),
    }
}

#[tauri::command]
pub(crate) fn restore_conversion(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<(), String> {
    let conversion = state
        .service
        .database()
        .get_conversion(id)
        .map_err(error_message)?;
    let stored = state
        .service
        .database()
        .stored_file(id)
        .map_err(error_message)?;
    let output_name = crate::conversion::pdf_file_name(&stored.source_name);
    let output_directory = conversion
        .source_path
        .as_deref()
        .and_then(|source| Path::new(source).parent())
        .filter(|directory| directory.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| state.application_home.join("legacy-restored-files"));
    let output_path = unique_output_path(&output_directory, id, &output_name);
    std::fs::create_dir_all(&output_directory)
        .map_err(|error| format!("Could not prepare the output folder: {error}"))?;
    std::fs::write(&output_path, stored.bytes)
        .map_err(|error| format!("Could not recreate the converted file: {error}"))?;
    state
        .service
        .database()
        .update_output_path(id, &output_path)
        .map_err(error_message)?;
    app.opener()
        .open_path(output_path.to_string_lossy(), None::<&str>)
        .map_err(error_message)
}

#[tauri::command]
pub(crate) fn delete_conversion(state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    if let Some(path) = state
        .service
        .database()
        .delete_conversion(id)
        .map_err(error_message)?
    {
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

fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
