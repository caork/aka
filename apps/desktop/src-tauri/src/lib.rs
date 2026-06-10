//! aka desktop shell.
//!
//! For now the frontend runs on mock data + file loading; real data will come
//! from aka-core / aka-search / aka-graph via Tauri commands in a later
//! milestone. The `ping` command exists so the IPC wiring can be smoke-tested.

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
