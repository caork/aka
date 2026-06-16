fn main() {
    println!("cargo:rerun-if-env-changed=AKA_ENABLE_NATIVE_UPDATER");
    let _ = std::fs::create_dir_all("resources/engine");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-env-changed=AKA_ENGINE_BIN_WIN_X64");
        println!("cargo:rerun-if-env-changed=AKA_CBM_BIN_WIN_X64");
        println!("cargo:rerun-if-changed=embedded-engine/aka-engine.exe");
        let dst = std::path::Path::new("embedded-engine").join("aka-engine.exe");
        let src = std::env::var("AKA_ENGINE_BIN_WIN_X64")
            .or_else(|_| std::env::var("AKA_CBM_BIN_WIN_X64"));
        if let Ok(src) = src {
            let _ = std::fs::create_dir_all("embedded-engine");
            if let Err(e) = std::fs::copy(&src, &dst) {
                panic!("copy embedded Windows engine from {src} to {}: {e}", dst.display());
            }
        } else {
            let legacy = std::path::Path::new("embedded-engine").join("codebase-memory-mcp.exe");
            if legacy.is_file() && !dst.is_file() {
                if let Err(e) = std::fs::copy(&legacy, &dst) {
                    panic!(
                        "copy legacy embedded Windows engine from {} to {}: {e}",
                        legacy.display(),
                        dst.display()
                    );
                }
            }
        }
        if !dst.is_file() {
            panic!(
                "missing embedded Windows engine at {}; set AKA_ENGINE_BIN_WIN_X64 before building",
                dst.display()
            );
        }
    }
    tauri_build::build()
}
