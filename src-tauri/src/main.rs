// Release builds do not open a black console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Guard process: does not start Tauri and does not touch single-instance.
    if shunguang_decor_lib::is_guard(&args) {
        std::process::exit(shunguang_decor_lib::run_guard(&args));
    }
    // Admin-privileged helper process (Hidden features): talks only over the pipe, no Tauri/window.
    if shunguang_decor_lib::is_elevated_helper(&args) {
        std::process::exit(shunguang_decor_lib::run_elevated_helper(&args));
    }
    // NSIS installer commands (src-tauri/windows/hooks.nsh): never open the app.
    if let Some(code) = shunguang_decor_lib::run_installer_command(&args) {
        std::process::exit(code);
    }
    shunguang_decor_lib::run();
}
