//! Thin shell. All logic lives in sleeve-core; the UI reaches it through one command.
use sleeve_core::Core;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command(async)]
fn call(core: tauri::State<'_, Arc<Core>>, cmd: String, args: serde_json::Value) -> Result<serde_json::Value, String> {
    core.dispatch(&cmd, &args).map_err(|e| e.to_string())
}

/// Folder suggested in the scan dialog.
#[tauri::command]
fn music_dir(app: tauri::AppHandle) -> String {
    if cfg!(target_os = "android") {
        return "/storage/emulated/0/Music".into();
    }
    let p = app.path();
    p.audio_dir().or_else(|_| p.document_dir()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_haptics::init());
    builder
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let core = Core::open(&dir.join("sleeve.db")).map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call, music_dir])
        .run(tauri::generate_context!())
        .expect("error while running Sleeve");
}
