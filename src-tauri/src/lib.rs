mod commands;
mod conversion;
mod database;

use std::{path::PathBuf, sync::Arc};

use conversion::ConversionService;
use database::Database;
#[cfg(not(dev))]
use tauri::{utils::config::FrontendDist, Url};
use tauri::{webview::WebviewWindowBuilder, Manager, WebviewUrl};

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
    let localhost_port =
        portpicker::pick_unused_port().expect("failed to find an unused localhost port");
    let localhost = tauri_plugin_localhost::Builder::new(localhost_port)
        .on_request(|_, response| {
            response.add_header("Cross-Origin-Opener-Policy", "same-origin");
            response.add_header("Cross-Origin-Embedder-Policy", "require-corp");
            response.add_header("Cross-Origin-Resource-Policy", "same-origin");
        })
        .build();

    #[cfg(dev)]
    let context = tauri::generate_context!();

    #[cfg(not(dev))]
    let mut context = tauri::generate_context!();

    #[cfg(not(dev))]
    let localhost_origin: Url = format!("http://localhost:{localhost_port}")
        .parse()
        .expect("failed to create the localhost URL");

    // The assets remain embedded by generate_context!, while this runtime URL
    // makes Tauri treat the exact random loopback origin as trusted for IPC.
    #[cfg(not(dev))]
    {
        context.config_mut().build.frontend_dist =
            Some(FrontendDist::Url(localhost_origin.clone()));
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(localhost)
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
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

            #[cfg(dev)]
            let window_url = WebviewUrl::App("/".into());

            #[cfg(not(dev))]
            let window_url = WebviewUrl::External(
                localhost_origin
                    .join("index.html")
                    .map_err(anyhow::Error::msg)?,
            );

            WebviewWindowBuilder::new(app, "main", window_url)
                .title("File Converter")
                .inner_size(1040.0, 720.0)
                .min_inner_size(760.0, 560.0)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_supported_formats,
            commands::inspect_source,
            commands::list_conversions,
            commands::get_history_entry,
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
        .run(context)
        .expect("error while running the file converter");
}
