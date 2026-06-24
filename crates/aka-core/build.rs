use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=AKA_ENGINE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=AKA_ENGINE_LINK_LIBGIT2");
    println!("cargo:rerun-if-env-changed=AKA_ENGINE_LINK_LIBGIT2_LIBS");

    if std::env::var_os("CARGO_FEATURE_EMBEDDED_ENGINE").is_none() {
        return;
    }

    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target == "windows" {
        println!("cargo:warning=aka-core embedded-engine is not linked on Windows yet; use the binary engine fallback");
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("aka-core lives under crates/aka-core");
    let default_lib_dir = workspace_dir.join("engine/aka-engine-src/build/c");
    let lib_dir = std::env::var_os("AKA_ENGINE_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_lib_dir);

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=aka_engine");
    if target == "macos" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=pthread");

    if std::env::var_os("AKA_ENGINE_LINK_LIBGIT2").is_some() {
        let libs = std::env::var("AKA_ENGINE_LINK_LIBGIT2_LIBS").unwrap_or_else(|_| "git2".into());
        for lib in libs.split(',').map(str::trim).filter(|lib| !lib.is_empty()) {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
