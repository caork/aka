use std::collections::BTreeSet;
use std::path::Path;

use super::source_scan::{normalize_repo_path, read_repo_text};

pub(super) fn discover_project_test_roots(
    repo: &Path,
    files: &BTreeSet<String>,
) -> BTreeSet<String> {
    let config_files = if files.is_empty() {
        discover_build_config_files(repo)
    } else {
        files
            .iter()
            .filter(|path| is_build_config_file(path))
            .cloned()
            .collect()
    };
    let mut roots = BTreeSet::new();
    for file_path in config_files {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let base_dir = Path::new(&file_path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("");
        for root in test_roots_from_build_config(&file_path, &text) {
            if let Some(root) = normalize_declared_source_root(base_dir, &root) {
                roots.insert(root);
            }
        }
    }
    roots
}

fn discover_build_config_files(repo: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(repo).ok().and_then(Path::to_str);
            let Some(rel) = rel.map(normalize_repo_path) else {
                continue;
            };
            let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
            if path.is_dir() {
                if !is_build_config_discovery_skip_dir(name) {
                    stack.push(path);
                }
            } else if is_build_config_file(&rel) {
                out.insert(rel);
            }
        }
    }
    out
}

fn is_build_config_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "pyproject.toml"
            | "setup.cfg"
            | "tox.ini"
            | "pytest.ini"
    )
}

fn is_build_config_discovery_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "target"
            | "build"
            | "dist"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
    )
}

fn test_roots_from_build_config(path: &str, text: &str) -> Vec<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "pom.xml" {
        return maven_test_roots(text);
    }
    if matches!(
        name,
        "build.gradle" | "build.gradle.kts" | "settings.gradle" | "settings.gradle.kts"
    ) {
        return gradle_test_roots(text);
    }
    if matches!(
        name,
        "pyproject.toml" | "setup.cfg" | "tox.ini" | "pytest.ini"
    ) {
        return python_test_roots(text);
    }
    Vec::new()
}

fn maven_test_roots(text: &str) -> Vec<String> {
    let mut roots = Vec::new();
    for tag in [
        "testSourceDirectory",
        "testOutputDirectory",
        "testClassesDirectory",
    ] {
        roots.extend(xml_tag_values(text, tag));
    }
    let lower = text.to_ascii_lowercase();
    let mut offset = 0usize;
    while let Some(pos) = lower[offset..].find("<testresource>") {
        let start = offset + pos;
        let Some(end_rel) = lower[start..].find("</testresource>") else {
            break;
        };
        roots.extend(xml_tag_values(&text[start..start + end_rel], "directory"));
        offset = start + end_rel + "</testresource>".len();
    }
    roots
}

fn xml_tag_values(text: &str, tag: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let open = format!("<{}>", tag.to_ascii_lowercase());
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = lower[offset..].find(&open) {
        let start = offset + pos + open.len();
        let Some(end_rel) = lower[start..].find(&close) else {
            break;
        };
        out.push(text[start..start + end_rel].trim().to_string());
        offset = start + end_rel + close.len();
    }
    out
}

fn gradle_test_roots(text: &str) -> Vec<String> {
    let mut roots = Vec::new();
    let mut recent_test_context = 0usize;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("sourcesets")
            || lower.contains("test {")
            || lower.contains("test{")
            || lower.contains("testfixtures")
        {
            recent_test_context = 8;
        }
        if (lower.contains("srcdir") || lower.contains("srcdirs"))
            && (recent_test_context > 0 || lower.contains("test"))
        {
            roots.extend(quoted_values(line).into_iter().filter(|root| {
                let root_lower = root.to_ascii_lowercase();
                root_lower.contains("test")
                    || root_lower.contains("spec")
                    || recent_test_context > 0
            }));
        }
        recent_test_context = recent_test_context.saturating_sub(1);
    }
    roots
}

fn python_test_roots(text: &str) -> Vec<String> {
    let mut roots = Vec::new();
    let mut in_pytest = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with('[') {
            in_pytest = lower.contains("pytest");
        }
        if !(in_pytest || lower.contains("pytest")) {
            continue;
        }
        if lower.starts_with("testpaths") || lower.starts_with("test_paths") {
            roots.extend(quoted_values(trimmed));
            if let Some((_, rhs)) = trimmed.split_once('=') {
                roots.extend(
                    rhs.split_whitespace()
                        .map(|value| value.trim_matches([',', '"', '\'']).to_string())
                        .filter(|value| !value.is_empty() && !value.starts_with('[')),
                );
            }
        }
    }
    roots
}

fn quoted_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"') {
            let quote = byte as char;
            let start = idx + 1;
            let mut end = start;
            while end < text.len() {
                let ch = text[end..].chars().next().unwrap_or_default();
                if ch == quote {
                    break;
                }
                end += ch.len_utf8();
            }
            if end < text.len() {
                out.push(text[start..end].to_string());
                idx = end + 1;
                continue;
            }
        }
        idx += 1;
    }
    out
}

fn normalize_declared_source_root(base_dir: &str, root: &str) -> Option<String> {
    let mut root = root.trim();
    if root.is_empty() {
        return None;
    }
    for prefix in [
        "${project.basedir}/",
        "${basedir}/",
        "$projectDir/",
        "$rootDir/",
    ] {
        if let Some(stripped) = root.strip_prefix(prefix) {
            root = stripped;
        }
    }
    let root = normalize_repo_path(root);
    if root.is_empty() || root.starts_with('$') || root.contains("${") {
        return None;
    }
    let combined = if base_dir.is_empty() || root.starts_with('/') {
        root
    } else {
        format!("{base_dir}/{root}")
    };
    Some(
        normalize_repo_path(&combined)
            .trim_end_matches('/')
            .to_string(),
    )
}
