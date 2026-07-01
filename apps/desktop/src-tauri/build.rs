fn main() {
    println!("cargo:rerun-if-env-changed=AKA_ENABLE_NATIVE_UPDATER");
    let _ = std::fs::create_dir_all("resources/engine");
    generate_embedded_client_integrations();
    tauri_build::build()
}

fn generate_embedded_client_integrations() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"),
    );
    let clients_dir = manifest_dir
        .join("..")
        .join("..")
        .join("..")
        .join("clients");
    println!("cargo:rerun-if-changed={}", clients_dir.display());

    let mut files = Vec::new();
    collect_files(&clients_dir, &clients_dir, &mut files)
        .unwrap_or_else(|err| panic!("collect client integration files failed: {err}"));
    files.sort();

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set"));
    let out_file = out_dir.join("embedded_client_integrations.rs");
    let mut generated =
        String::from("pub const EMBEDDED_CLIENT_INTEGRATION_FILES: &[(&str, &[u8])] = &[\n");
    for rel in files {
        println!(
            "cargo:rerun-if-changed={}",
            clients_dir.join(&rel).display()
        );
        generated.push_str("    (");
        generated.push_str(&format!("{rel:?}"));
        generated.push_str(
            ", include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../../clients/",
        );
        generated.push_str(&rel);
        generated.push_str("\"))),\n");
    }
    generated.push_str("];\n");
    std::fs::write(out_file, generated)
        .unwrap_or_else(|err| panic!("write embedded client integrations failed: {err}"));
}

fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    println!("cargo:rerun-if-changed={}", dir.display());
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
        } else if file_type.is_file() && entry.file_name() != ".DS_Store" {
            let rel = path.strip_prefix(root).expect("path is under root");
            files.push(
                rel.components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    Ok(())
}
