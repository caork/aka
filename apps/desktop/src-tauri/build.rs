fn main() {
    let _ = std::fs::create_dir_all("resources/engine");
    tauri_build::build()
}
