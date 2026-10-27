pub mod app;
pub mod calibration;
pub mod commands;
pub mod gcode;
pub mod machine;
pub mod marlin;
pub mod planner;
pub mod safety;
pub mod serial;
pub mod sim;
pub mod transport;

use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            let handle = app.handle().clone();
            let emit: app::Emit = Arc::new(move |name, payload| {
                let _ = handle.emit(name, payload);
            });
            app.manage(app::AppState::new(dir, emit));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_serial_ports,
            commands::check_port,
            commands::connect_simulation,
            commands::connect_hardware,
            commands::disconnect,
            commands::machine_state,
            commands::recent_log,
            commands::arm,
            commands::disarm,
            commands::jog,
            commands::goto_position,
            commands::home,
            commands::read_position,
            commands::verify_position,
            commands::pause_job,
            commands::resume_job,
            commands::cancel_job,
            commands::quick_stop,
            commands::emergency_stop,
            commands::get_profiles,
            commands::select_profile,
            commands::update_profile,
            commands::set_calibration,
            commands::calibration_status,
            commands::plan_preview,
            commands::create_job,
            commands::list_jobs,
            commands::job_detail,
            commands::delete_job,
            commands::job_gcode,
            commands::export_job,
            commands::start_job,
            commands::sim_state,
            commands::sim_trail,
            commands::sim_fault,
            commands::sim_speed,
            commands::sim_clear,
            commands::sim_set_truth,
            commands::get_settings,
            commands::set_setting,
            commands::read_text_file,
            commands::write_text_file,
            commands::read_binary_file,
            commands::list_system_fonts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CtoP");
}
