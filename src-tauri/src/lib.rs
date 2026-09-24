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
            let _ = services::task_store::recover_interrupted();
            let _ = services::task_store::bootstrap_existing_tasks();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pipeline::prepare_job,
            commands::pipeline::render_job,
            commands::pipeline::load_opening_sample,
            commands::pipeline::read_sample_reviews,
            commands::pipeline::generate_opening_sample,
            commands::pipeline::render_opening_sample,
            commands::pipeline::save_edit_plan,
            commands::pipeline::export_commentary_script,
            commands::pipeline::load_job,
            commands::pipeline::list_existing_jobs,
            commands::pipeline::read_job_transcript,
            commands::pipeline::refetch_platform_subtitle_and_generate,
            commands::pipeline::delete_job,
            commands::pipeline::generate_director_plan,
            commands::pipeline::reveal_path,
            commands::settings::get_settings,
            commands::settings::preview_edge_voice,
            commands::settings::save_settings,
            commands::settings::check_dependencies,
            commands::settings::check_ollama,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
