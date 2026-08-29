mod database;
mod gateway;
mod secrets;

use std::path::{Path, PathBuf};

use database::{Conversion, Database, PublicSettings};
use gateway::GatewayClient;
use secrets::SecretStore;
use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;
use url::Url;
use uuid::Uuid;

struct AppState {
    database: Database,
    output_directory: PathBuf,
    gateway: GatewayClient,
    secrets: SecretStore,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenConversionResult {
    missing: bool,
    restorable: bool,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<PublicSettings, String> {
    let token_configured = state
        .secrets
        .gateway_token_configured()
        .map_err(error_message)?;
    state
        .database
        .public_settings(token_configured)
        .map_err(error_message)
}

#[tauri::command]
fn save_settings(
    state: State<'_, AppState>,
    gateway_url: String,
    gateway_token: Option<String>,
) -> Result<PublicSettings, String> {
    validate_gateway_url(&gateway_url)?;
    if let Some(token) = gateway_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        state
            .secrets
            .set_gateway_token(token)
            .map_err(error_message)?;
    }
    state
        .database
        .save_gateway_url(gateway_url.trim_end_matches('/'))
        .map_err(error_message)?;
    get_settings(state)
}

#[tauri::command]
fn list_conversions(state: State<'_, AppState>) -> Result<Vec<Conversion>, String> {
    state.database.list_conversions().map_err(error_message)
}

#[tauri::command]
async fn start_conversion(
    state: State<'_, AppState>,
    input_path: String,
) -> Result<Conversion, String> {
    let path = PathBuf::from(&input_path);
    let source_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The selected file does not have a valid name.".to_string())?
        .to_string();
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| format!("Could not read {source_name}: {error}"))?;
    let gateway_token = state.secrets.gateway_token().map_err(error_message)?;
    let settings = state
        .database
        .private_settings(gateway_token)
        .map_err(error_message)?;
    let id = state
        .database
        .insert_conversion(Uuid::new_v4(), &source_name, "pdf")
        .map_err(error_message)?
        .id;
    let output_name = pdf_file_name(&source_name);
    let output_path = unique_output_path(&state.output_directory, id, &output_name);

    let result = async {
        let converted = state
            .gateway
            .convert_to_pdf(&settings, &source_name, bytes)
            .await
            .map_err(error_message)?;
        tokio::fs::write(&output_path, &converted)
            .await
            .map_err(|error| format!("Could not save the converted PDF: {error}"))?;
        state
            .database
            .mark_finished(id, &output_path, &converted)
            .map_err(error_message)
    }
    .await;

    match result {
        Ok(conversion) => Ok(conversion),
        Err(error) => state
            .database
            .mark_failed(id, &error)
            .map_err(error_message),
    }
}

#[tauri::command]
fn open_conversion(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<OpenConversionResult, String> {
    let conversion = state.database.get_conversion(id).map_err(error_message)?;
    let path = conversion
        .output_path
        .ok_or_else(|| "This conversion does not have a downloaded file yet.".to_string())?;
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            app.opener()
                .open_path(path, None::<&str>)
                .map_err(error_message)?;
            Ok(OpenConversionResult {
                missing: false,
                restorable: false,
            })
        }
        Ok(_) => Err("The saved output path is not a file.".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let restorable = state.database.has_stored_file(id).map_err(error_message)?;
            Ok(OpenConversionResult {
                missing: true,
                restorable,
            })
        }
        Err(error) => Err(format!("Could not check the converted file: {error}")),
    }
}

#[tauri::command]
fn restore_conversion(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<(), String> {
    let stored = state.database.stored_file(id).map_err(error_message)?;
    let output_name = pdf_file_name(&stored.source_name);
    let output_path = unique_output_path(&state.output_directory, id, &output_name);
    std::fs::create_dir_all(&state.output_directory)
        .map_err(|error| format!("Could not prepare the converted-files folder: {error}"))?;
    std::fs::write(&output_path, stored.bytes)
        .map_err(|error| format!("Could not recreate the converted file: {error}"))?;
    state
        .database
        .update_output_path(id, &output_path)
        .map_err(error_message)?;
    app.opener()
        .open_path(output_path.to_string_lossy(), None::<&str>)
        .map_err(error_message)
}

#[tauri::command]
fn delete_conversion(state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    if let Some(path) = state
        .database
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

fn validate_gateway_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| "Enter a complete gateway URL.".to_string())?;
    let local_http = url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !local_http {
        return Err("The gateway must use HTTPS (HTTP is allowed only for localhost).".into());
    }
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err("Enter a valid gateway URL.".into());
    }
    Ok(())
}

fn pdf_file_name(value: &str) -> String {
    let stem = Path::new(value)
        .file_name()
        .and_then(|name| Path::new(name).file_stem())
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("converted-file");
    format!("{stem}.pdf")
}

fn unique_output_path(directory: &Path, id: Uuid, file_name: &str) -> PathBuf {
    directory.join(format!("{}-{file_name}", &id.to_string()[..8]))
}

fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|error| anyhow::anyhow!(error))?;
            let output_directory = app_data.join("converted-files");
            std::fs::create_dir_all(&output_directory)?;
            let database = Database::open(&app_data.join("file-converter.sqlite3"))?;
            database.fail_interrupted_conversions()?;
            database.migrate_legacy_base64_files()?;
            database.backfill_stored_files()?;
            let secrets = SecretStore::new();
            if let Some(legacy_token) = database.legacy_gateway_token()? {
                if !secrets.gateway_token_configured()? {
                    secrets.set_gateway_token(&legacy_token)?;
                }
                database.delete_legacy_gateway_token()?;
            }
            app.manage(AppState {
                database,
                output_directory,
                gateway: GatewayClient::new()?,
                secrets,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_conversions,
            start_conversion,
            open_conversion,
            restore_conversion,
            delete_conversion,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the file converter");
}

#[cfg(test)]
mod tests {
    use super::{pdf_file_name, validate_gateway_url};

    #[test]
    fn gateway_requires_tls_except_during_local_development() {
        assert!(validate_gateway_url("https://converter.example.com").is_ok());
        assert!(validate_gateway_url("http://localhost:8080").is_ok());
        assert!(validate_gateway_url("http://converter.example.com").is_err());
    }

    #[test]
    fn output_names_are_pdf_files() {
        assert_eq!(pdf_file_name("letter.docx"), "letter.pdf");
        assert_eq!(pdf_file_name("../../slides.pptx"), "slides.pdf");
    }
}
