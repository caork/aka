fn main() {
    let _ = std::fs::create_dir_all("resources/engine");
    prepare_client_integrations();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-env-changed=AKA_CBM_BIN_WIN_X64");
        println!("cargo:rerun-if-changed=embedded-engine/codebase-memory-mcp.exe");
        let dst = std::path::Path::new("embedded-engine").join("codebase-memory-mcp.exe");
        if let Ok(src) = std::env::var("AKA_CBM_BIN_WIN_X64") {
            let _ = std::fs::create_dir_all("embedded-engine");
            if let Err(e) = std::fs::copy(&src, &dst) {
                panic!(
                    "copy embedded Windows engine from {src} to {}: {e}",
                    dst.display()
                );
            }
        }
        if !dst.is_file() {
            panic!(
                "missing embedded Windows engine at {}; set AKA_CBM_BIN_WIN_X64 before building",
                dst.display()
            );
        }
    }
    tauri_build::build()
}

fn prepare_client_integrations() {
    println!("cargo:rerun-if-changed=../../../clients");
    println!("cargo:rerun-if-changed=../../../.claude-plugin");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let dst = std::path::Path::new("resources").join("client-integrations");
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst).expect("create client integration resources dir");
    copy_dir(&root.join("clients"), &dst.join("clients"))
        .expect("copy bundled client integrations");
    copy_dir(&root.join(".claude-plugin"), &dst.join(".claude-plugin"))
        .expect("copy bundled Claude marketplace metadata");
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&source, &target)?;
        } else if file_type.is_file() {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}
