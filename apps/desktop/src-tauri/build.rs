fn main() {
    println!("cargo:rerun-if-env-changed=AKA_ENABLE_NATIVE_UPDATER");
    let _ = std::fs::create_dir_all("resources/engine");
    tauri_build::build()
}
