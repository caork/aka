// Prevents an extra console window when the desktop GUI is launched on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    if should_run_cli_mode() {
        aka_cli::commands::run_from_env()
    } else {
        aka_desktop_lib::run();
        Ok(())
    }
}

fn should_run_cli_mode() -> bool {
    let mut args = std::env::args_os();
    let _exe = args.next();
    matches!(
        args.next()
            .and_then(|arg| arg.into_string().ok())
            .as_deref(),
        Some(
            "analyze"
                | "index"
                | "repos"
                | "search"
                | "search-code"
                | "context"
                | "lod"
                | "mcp"
                | "serve"
                | "help"
                | "--help"
                | "-h"
                | "--version"
                | "-V"
        )
    )
}
