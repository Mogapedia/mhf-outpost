mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_versions,
            commands::get_version_info,
            commands::run_checks,
            commands::download_version,
            commands::launch_game,
            commands::fetch_launcher,
            commands::av_exclude,
            commands::authenticate,
            commands::select_character,
            commands::launch_game_authed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
