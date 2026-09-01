mod commands;
mod conversion;
mod database;

use std::{path::PathBuf, sync::Arc};

use conversion::ConversionService;
use database::Database;
use tauri::Manager;

struct AppState {
    service: ConversionService,
    application_home: PathBuf,
}

fn application_home(app: &tauri::AppHandle) -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(app
            .path()
            .home_dir()
            .map_err(anyhow::Error::msg)?
            .join("Library/Application Support/FileConverter"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let identifier_directory = app.path().app_data_dir().map_err(anyhow::Error::msg)?;
        let data_root = identifier_directory.parent().ok_or_else(|| {
            anyhow::anyhow!("the operating system application-data path has no parent")
        })?;
        Ok(data_root.join("FileConverter"))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let legacy_home = app.path().app_data_dir().map_err(anyhow::Error::msg)?;
            let application_home = application_home(app.handle())?;
            std::fs::create_dir_all(&application_home)?;
            let database_path = application_home.join("file-converter.sqlite3");
            Database::migrate_if_missing(
                &legacy_home.join("file-converter.sqlite3"),
                &database_path,
            )?;
            let work_root = application_home.join("cache/conversion-work");
            let database = Arc::new(Database::open(&database_path)?);
            database.fail_interrupted_conversions()?;
            let service = ConversionService::new(database, work_root);
            service.cleanup_stale_workdirs()?;
            app.manage(AppState {
                service,
                application_home,
            });
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
