use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command as GitCommand;

use super::build_config_scan::discover_project_test_roots;
use super::SynthNode;

pub(super) const JVM_SOURCE_EXTENSIONS: &[&str] = &["java", "kt", "kts", "scala", "groovy"];
pub(super) const PYTHON_SOURCE_EXTENSIONS: &[&str] = &["py"];
pub(super) const BUSINESS_SOURCE_EXTENSIONS: &[&str] =
    &["java", "kt", "kts", "scala", "groovy", "py"];

#[derive(Debug, Clone)]
pub(super) struct ProjectSourceSet {
    files: BTreeSet<String>,
    has_git_listing: bool,
    tracked_files: BTreeSet<String>,
    untracked_files: BTreeSet<String>,
    test_roots: BTreeSet<String>,
}

impl ProjectSourceSet {
    pub(super) fn discover(repo: &Path) -> Self {
        let git_files = git_project_files(repo);
        let has_git_listing = git_files.is_some();
        let (files, tracked_files, untracked_files) = if let Some(git_files) = git_files {
            let files = git_files
                .tracked
                .union(&git_files.untracked)
                .cloned()
                .collect();
            (files, git_files.tracked, git_files.untracked)
        } else {
            (discover_repo_files(repo), BTreeSet::new(), BTreeSet::new())
        };
        let test_roots = discover_project_test_roots(repo, &files);
        Self {
            has_git_listing,
            files,
            tracked_files,
            untracked_files,
            test_roots,
        }
    }

    pub(super) fn contains_project_file(&self, repo: &Path, file_path: &str) -> bool {
        let normalized = normalize_repo_path(file_path);
        if normalized.is_empty()
            || is_noisy_source_path(&normalized)
            || self.is_project_test_root_file(&normalized)
        {
            return false;
        }
        if self.has_git_listing {
            return repo.join(&normalized).is_file()
                && (self.tracked_files.contains(&normalized)
                    || self.untracked_files.contains(&normalized));
        }
        repo.join(&normalized).is_file()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    pub(super) fn project_files<'a>(
        &'a self,
        repo: &'a Path,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.files
            .iter()
            .map(String::as_str)
            .filter(move |path| self.contains_project_file(repo, path))
    }

    pub(super) fn project_files_with_extensions<'a>(
        &'a self,
        repo: &'a Path,
        extensions: &'a [&'a str],
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.project_files(repo).filter(move |path| {
            Path::new(&path.to_ascii_lowercase())
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
        })
    }

    pub(super) fn project_jvm_source_files<'a>(
        &'a self,
        repo: &'a Path,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.project_files_with_extensions(repo, JVM_SOURCE_EXTENSIONS)
    }

    pub(super) fn project_python_source_files<'a>(
        &'a self,
        repo: &'a Path,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.project_files_with_extensions(repo, PYTHON_SOURCE_EXTENSIONS)
    }

    pub(super) fn has_git_listing(&self) -> bool {
        self.has_git_listing
    }

    #[cfg(test)]
    fn is_git_tracked_file(&self, file_path: &str) -> bool {
        self.tracked_files.contains(&normalize_repo_path(file_path))
    }

    #[cfg(test)]
    fn is_git_untracked_file(&self, file_path: &str) -> bool {
        self.untracked_files
            .contains(&normalize_repo_path(file_path))
    }

    fn is_project_test_root_file(&self, file_path: &str) -> bool {
        self.test_roots
            .iter()
            .any(|root| path_is_within_dir(file_path, root))
    }
}

#[derive(Debug)]
struct GitProjectFiles {
    tracked: BTreeSet<String>,
    untracked: BTreeSet<String>,
}

fn git_project_files(repo: &Path) -> Option<GitProjectFiles> {
    Some(GitProjectFiles {
        tracked: git_ls_files(repo, &["--cached"])?,
        untracked: git_ls_files(repo, &["--others", "--exclude-standard"])?,
    })
}

fn git_ls_files(repo: &Path, args: &[&str]) -> Option<BTreeSet<String>> {
    let Ok(output) = GitCommand::new("git")
        .arg("-C")
        .arg(repo)
        .arg("ls-files")
        .arg("-z")
        .args(args)
        .output()
    else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .map(normalize_repo_path)
            .filter(|path| !path.is_empty() && !is_noisy_source_path(path))
            .collect(),
    )
}

fn discover_repo_files(repo: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_repo_files(repo, repo, &mut out);
    out
}

fn collect_repo_files(repo: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(repo)
            .ok()
            .and_then(Path::to_str)
            .map(normalize_repo_path);
        let Some(rel) = rel else {
            continue;
        };
        if rel.is_empty() || is_noisy_source_path(&rel) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_repo_files(repo, &path, out);
        } else if file_type.is_file() {
            out.insert(rel);
        }
    }
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
    Path::new(&file_path.to_ascii_lowercase())
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| BUSINESS_SOURCE_EXTENSIONS.contains(&ext))
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

pub(super) fn stable_hash(s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo(tag: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aka-source-scan-{tag}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = GitCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn project_source_set_uses_tracked_files_with_untracked_overlay() {
        let repo = temp_repo("git-overlay");
        run_git(&repo, &["init"]);
        std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
        std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
        std::fs::create_dir_all(repo.join("scratch")).unwrap();
        std::fs::write(repo.join(".gitignore"), "scratch/\n").unwrap();
        std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
        let tracked = "src/main/java/com/example/ops/TrackedMaintenance.java";
        let untracked = "src/main/java/com/example/ops/UntrackedMaintenance.java";
        let test = "src/test/java/com/example/ops/TestMaintenance.java";
        let ignored = "scratch/IgnoredMaintenance.java";
        for file in [tracked, untracked, test, ignored] {
            std::fs::write(repo.join(file), "class Maintenance {}\n").unwrap();
        }
        run_git(&repo, &["add", ".gitignore", "pom.xml", tracked, test]);

        let sources = ProjectSourceSet::discover(&repo);

        assert!(sources.has_git_listing());
        assert!(sources.is_git_tracked_file(tracked));
        assert!(sources.is_git_untracked_file(untracked));
        assert!(sources.contains_project_file(&repo, tracked));
        assert!(sources.contains_project_file(&repo, untracked));
        assert!(!sources.contains_project_file(&repo, test));
        assert!(!sources.contains_project_file(&repo, ignored));
    }

    #[test]
    fn project_source_set_ignores_deleted_tracked_files() {
        let repo = temp_repo("git-deleted-tracked");
        run_git(&repo, &["init"]);
        std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
        std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
        let deleted = "src/main/java/com/example/ops/DeletedMaintenance.java";
        std::fs::write(repo.join(deleted), "class DeletedMaintenance {}\n").unwrap();
        run_git(&repo, &["add", "pom.xml", deleted]);
        std::fs::remove_file(repo.join(deleted)).unwrap();

        let sources = ProjectSourceSet::discover(&repo);

        assert!(sources.has_git_listing());
        assert!(sources.is_git_tracked_file(deleted));
        assert!(!sources.contains_project_file(&repo, deleted));
    }
}
