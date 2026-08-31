mod commands;
mod conversion;
mod database;

use std::sync::Arc;

use conversion::ConversionService;
use database::Database;
use tauri::Manager;

struct AppState {
    service: ConversionService,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data = app.path().app_data_dir().map_err(anyhow::Error::msg)?;
            let work_root = app
                .path()
                .app_cache_dir()
                .map_err(anyhow::Error::msg)?
                .join("conversion-work");
            let database = Arc::new(Database::open(&app_data.join("file-converter.sqlite3"))?);
            database.fail_interrupted_conversions()?;
            let service = ConversionService::new(database, work_root);
            service.cleanup_stale_workdirs()?;
            app.manage(AppState { service });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_supported_formats,
            commands::inspect_source,
            commands::list_conversions,
            commands::start_conversion,
            commands::reconvert,
            commands::read_conversion_input,
            commands::complete_wasm_conversion,
            commands::fail_wasm_conversion,
            commands::cancel_conversion,
            commands::open_conversion,
            commands::restore_conversion,
            commands::delete_conversion,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the file converter");
}
