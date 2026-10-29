#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod gui;

fn main() {
    let arguments = std::env::args().skip(1).collect();
    if let Err(_error) = gui::run(arguments) {
        #[cfg(not(target_os = "windows"))]
        eprintln!("ClipSync GUI failed: {_error}");
    }
}
