mod commands;
pub mod sync;
pub mod model;

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(commands::handler())
        .run(tauri::generate_context!())
        .expect("failed to run Kite");
}
