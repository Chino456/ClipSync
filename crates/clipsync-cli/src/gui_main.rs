#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod gui;

fn main() {
    if let Err(_error) = gui::run() {
        #[cfg(not(target_os = "windows"))]
        eprintln!("ClipSync GUI failed: {_error}");
    }
}
