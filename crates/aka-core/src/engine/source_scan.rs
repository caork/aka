use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command as GitCommand;

use super::build_config_scan::discover_project_test_roots;
use super::SynthNode;

#[derive(Debug, Clone)]
pub(super) struct ProjectSourceSet {
    files: BTreeSet<String>,
    has_git_listing: bool,
    test_roots: BTreeSet<String>,
}

impl ProjectSourceSet {
    pub(super) fn discover(repo: &Path) -> Self {
        let files = git_project_files(repo);
        let test_roots = discover_project_test_roots(repo, &files);
        Self {
            has_git_listing: !files.is_empty(),
            files,
            test_roots,
        }
    }

    pub(super) fn contains_project_file(&self, repo: &Path, file_path: &str) -> bool {
        let normalized = normalize_repo_path(file_path);
        if normalized.is_empty()
            || is_noisy_source_path(&normalized)
            || is_project_test_source_path(&normalized)
            || self.is_project_test_root_file(&normalized)
        {
            return false;
        }
        if self.has_git_listing {
            return self.files.contains(&normalized);
        }
        repo.join(&normalized).is_file()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    pub(super) fn has_git_listing(&self) -> bool {
        self.has_git_listing
    }

    fn is_project_test_root_file(&self, file_path: &str) -> bool {
        self.test_roots
            .iter()
            .any(|root| path_is_within_dir(file_path, root))
    }
}

fn git_project_files(repo: &Path) -> BTreeSet<String> {
    let Ok(output) = GitCommand::new("git")
        .arg("-C")
        .arg(repo)
        .arg("ls-files")
        .arg("-z")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .output()
    else {
        return BTreeSet::new();
    };
    if !output.status.success() {
        return BTreeSet::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .map(normalize_repo_path)
        .filter(|path| !path.is_empty())
        .collect()
}

fn path_is_within_dir(path: &str, dir: &str) -> bool {
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(super) struct CallArgs<'a> {
    pub(super) start: usize,
    pub(super) args: &'a str,
}

pub(super) fn find_call_args<'a>(text: &'a str, callee: &str) -> Vec<CallArgs<'a>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(callee) {
        let start = offset + rel;
        if !call_name_boundary_ok(text, start, callee) {
            offset = start + callee.len();
            continue;
        }
        let open = skip_ws(text, start + callee.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + callee.len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.push(CallArgs {
            start,
            args: &text[open + 1..close],
        });
        offset = close + 1;
    }
    out
}

fn call_name_boundary_ok(text: &str, start: usize, callee: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + callee.len())
        .copied()
        .map(char::from);
    let before_ok = if callee.starts_with('.') {
        before.is_some_and(is_ident_continue)
    } else {
        before.is_none_or(|ch| !is_ident_continue(ch) && ch != '.')
    };
    before_ok && after.is_none_or(|ch| !is_ident_continue(ch))
}

pub(super) fn find_matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (idx, byte) in text.bytes().enumerate().skip(open) {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == q {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn split_top_level_commas(args: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    let mut start = 0usize;
    for (idx, byte) in args.bytes().enumerate() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == q {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&args[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out
}

pub(super) fn node_at_offset<'a>(
    text: &str,
    nodes: &'a [&'a SynthNode],
    offset: usize,
) -> Option<&'a SynthNode> {
    let line = line_number_at_offset(text, offset);
    nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        .filter(|node| {
            let start = node.start_line_key().max(1);
            let end = node.end_line_key().max(start);
            line >= start && line <= end
        })
        .max_by_key(|node| node.start_line_key())
}

fn line_number_at_offset(text: &str, offset: usize) -> i64 {
    let bounded = offset.min(text.len());
    let mut line = 1i64;
    for (idx, ch) in text.char_indices() {
        if idx >= bounded {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line
}

pub(super) fn nodes_by_file(
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<&SynthNode>> {
    let mut by_file: BTreeMap<String, Vec<&SynthNode>> = BTreeMap::new();
    for node in nodes.values() {
        if node.file_path.is_empty() || is_noisy_source_path(&node.file_path) {
            continue;
        }
        by_file
            .entry(node.file_path.clone())
            .or_default()
            .push(node);
    }
    for file_nodes in by_file.values_mut() {
        file_nodes.sort_by(|a, b| {
            handler_rank(a)
                .cmp(&handler_rank(b))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.aka_id.cmp(&b.aka_id))
        });
    }
    by_file
}

pub(super) fn project_code_nodes_by_file<'a>(
    repo: &Path,
    nodes: &'a BTreeMap<String, SynthNode>,
    project_sources: &ProjectSourceSet,
) -> BTreeMap<String, Vec<&'a SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            project_sources.contains_project_file(repo, file_path)
                && (is_project_code_source_path(file_path)
                    || file_nodes
                        .iter()
                        .any(|node| is_business_language(&node.language)))
        })
        .collect()
}

pub(super) fn is_project_code_source_path(file_path: &str) -> bool {
    matches!(
        Path::new(&file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
    )
}

pub(super) fn is_business_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "java" | "kotlin" | "scala" | "groovy" | "python"
    )
}

pub(super) fn read_repo_text(repo: &Path, file_path: &str) -> Option<String> {
    std::fs::read_to_string(repo.join(file_path)).ok()
}

pub(super) fn normalize_repo_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub(super) fn pick_handler_node<'a>(nodes: &'a [&'a SynthNode]) -> Option<&'a SynthNode> {
    nodes
        .iter()
        .copied()
        .find(|n| matches!(n.label.as_str(), "Function" | "Method") && handler_rank(n) <= 1)
        .or_else(|| {
            nodes
                .iter()
                .copied()
                .find(|n| matches!(n.label.as_str(), "Function" | "Method"))
        })
        .or_else(|| nodes.first().copied())
}

fn handler_rank(node: &SynthNode) -> u8 {
    let lower = node.name.to_ascii_lowercase();
    if lower == "handler" || lower == "handle" || lower.starts_with("handle") {
        0
    } else if matches!(
        lower.as_str(),
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    ) || lower.ends_with("handler")
        || lower.ends_with("controller")
    {
        1
    } else if matches!(node.label.as_str(), "Function" | "Method") {
        2
    } else {
        3
    }
}

pub(super) fn skip_ws(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

pub(super) fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

pub(super) fn is_noisy_source_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.split('/').any(|segment| {
        matches!(
            segment,
            "node_modules"
                | "vendor"
                | "vendors"
                | "dist"
                | "build"
                | "target"
                | "coverage"
                | "__pycache__"
                | ".venv"
                | "venv"
                | "generated"
                | "third_party"
                | "third-party"
        )
    }) || path.ends_with(".min.js")
}

pub(super) fn is_project_test_source_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(path.as_str());
    path.contains(".test.")
        || path.contains(".spec.")
        || path.contains("/src/test/")
        || path.contains("/src/it/")
        || path.contains("/src/integrationtest/")
        || path.contains("/src/e2e/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/testing/")
        || path.contains("/__tests__/")
        || path.contains("/__mocks__/")
        || path.contains("/spec/")
        || path.contains(".tests/")
        || path.contains(".test/")
        || path.contains("uitests/")
        || name.starts_with("test_")
        || name == "conftest.py"
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with("tests.swift")
        || name.ends_with("test.swift")
        || name.ends_with("tests.cs")
        || name.ends_with("test.cs")
        || name.ends_with("test.php")
        || name.ends_with("spec.php")
        || name.ends_with("_spec.rb")
        || name.ends_with("_test.rb")
}

pub(super) fn stable_hash(s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
