mod commands;
mod models;
mod services;
mod state;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(AppState::new(handle));
            let inbox = services::jobs::inbox_root();
            app.asset_protocol_scope().allow_directory(&inbox, true)?;
            // Tauri excludes dot-directories from recursive globs on macOS.
            app.asset_protocol_scope()
                .allow_directory(inbox.join(".voice-previews"), true)?;
            let _ = services::task_store::recover_interrupted();
            let _ = services::task_store::bootstrap_existing_tasks();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pipeline::prepare_job,
            commands::pipeline::list_existing_jobs,
            commands::pipeline::read_job_transcript,
            commands::pipeline::refetch_subtitles,
            commands::pipeline::delete_job,
            commands::pipeline::reveal_path,
            commands::episode::load_episode,
            commands::episode::generate_episode,
            commands::episode::save_episode,
            commands::episode::review_episode,
            commands::episode::render_episode,
            commands::shorts::load_shorts,
            commands::shorts::generate_shorts,
            commands::shorts::rewrite_short,
            commands::shorts::render_short,
            commands::settings::get_settings,
            commands::settings::preview_local_voice,
            commands::settings::list_local_voices,
            commands::settings::save_settings,
            commands::settings::check_dependencies,
            commands::settings::check_ai,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
