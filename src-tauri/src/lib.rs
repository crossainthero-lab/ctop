pub mod app;
pub mod calibration;
pub mod commands;
pub mod ctl;
pub mod diag;
pub mod fonts;
pub mod frame;
pub mod gcode;
pub mod geom;
pub mod machine;
pub mod marlin;
pub mod paper;
pub mod planner;
pub mod safety;
pub mod serial;
pub mod sim;
pub mod transport;
pub mod web;
pub mod writer;

use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;

/// Where CtoP keeps its data: `$CTOP_DATA`, else the same directory the desktop app uses.
pub fn data_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("CTOP_DATA") {
        return d.into();
    }
    let base = std::env::var("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share"));
    base.join("dev.ctop.app")
}

/// Command used by both the desktop window and the browser service.
#[tauri::command(async)]
fn call(w: tauri::State<Arc<writer::Writer>>, cmd: String, args: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    writer::call(&w, &cmd, args.unwrap_or(serde_json::Value::Null))
}

fn web_port() -> u16 {
    std::env::var("CTOP_WEB_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(web::DEFAULT_PORT)
}

/// Headless mode: `ctop --serve` runs the same backend without a window and serves the browser UI on 127.0.0.1 (set CTOP_WEB_BIND=0.0.0.0 for LAN access).
pub fn serve() -> Result<(), String> {
    diag::install_panic_hook();
    let dir = data_dir();
    let emit: app::Emit = Arc::new(|_, _| {});
    let ctl = ctl::Ctl::new(dir.clone(), None, None, emit);
    let w = writer::Writer::new(dir.clone(), ctl);
    w.start();
    let url = web::start(w, &dir, web_port())?;
    println!("CtoP browser UI: {url}");
    println!("(keep this address private: the token in it controls the plotter)");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    diag::install_panic_hook();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = if std::env::var("CTOP_DATA").is_ok() { data_dir() } else { app.path().app_data_dir().expect("app data dir") };
            let handle = app.handle().clone();
            let emit: app::Emit = Arc::new(move |name, payload| {
                let _ = handle.emit(name, payload);
            });
            let state = app::AppState::new(dir.clone(), emit.clone());
            let st = state.settings();
            let gets = |k: &str| st.get(k).and_then(|v| v.as_str()).map(String::from);
            let ctl = ctl::Ctl::new(dir.clone(), gets("controllerSocket"), gets("controllerDir"), emit);
            let w = writer::Writer::new(dir.clone(), ctl.clone());
            w.start();
            if std::env::var("CTOP_WEB").map(|v| v == "1").unwrap_or(false) || st.get("webEnabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                match web::start(w.clone(), &dir, web_port()) {
                    Ok(url) => println!("CtoP browser UI: {url}"),
                    Err(e) => diag::error("web", e),
                }
            }
            app.manage(state);
            app.manage(ctl);
            app.manage(w);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            call,
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
            commands::ctl_snapshot,
            commands::ctl_set_config,
            commands::ctl_sheets,
            commands::ctl_profiles,
            commands::ctl_confirm_sheet,
            commands::ctl_prepare,
            commands::ctl_start,
            commands::ctl_pause,
            commands::ctl_resume,
            commands::ctl_stop,
            commands::ctl_estop,
            commands::ctl_discard_run,
            commands::ctl_jog,
            commands::ctl_goto,
            commands::ctl_pen_up,
            commands::ctl_z_step,
            commands::service_up,
            commands::start_worksheets,
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
