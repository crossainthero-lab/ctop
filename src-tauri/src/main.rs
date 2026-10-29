// Prevents an extra console window on Windows release builds; harmless on Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|a| a == "--serve") {
        if let Err(e) = ctop_lib::serve() {
            eprintln!("ctop: {e}");
            std::process::exit(1);
        }
        return;
    }
    ctop_lib::run()
}
