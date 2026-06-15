use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::{
    find_call_args, read_repo_text, read_string_literal, split_top_level_commas, ProjectSourceSet,
    RouteCandidate, SynthNode,
};

pub(super) fn django_urlconf_routes_from_repo(
    repo: &Path,
    project_sources: &ProjectSourceSet,
    by_file: &BTreeMap<String, Vec<&SynthNode>>,
) -> BTreeMap<String, Vec<RouteCandidate>> {
    let handlers = PythonHandlerIndex::new(by_file);
    let mut out: BTreeMap<String, Vec<RouteCandidate>> = BTreeMap::new();
    let file_paths = django_urlconf_files(repo, project_sources);
    for file_path in file_paths {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let mut candidates = django_urlconf_routes_with_handlers(&text, &handlers);
        if candidates.is_empty() {
            continue;
        }
        dedup_candidates(&mut candidates);
        out.insert(file_path.to_string(), candidates);
    }
    out
}

pub(super) fn django_urlconf_routes(
    file_path: &str,
    text: &str,
    by_file: &BTreeMap<String, Vec<&SynthNode>>,
) -> Vec<RouteCandidate> {
    if !file_path.to_ascii_lowercase().ends_with(".py")
        || !is_django_urlconf_text(text)
        || !(text.contains("path(") || text.contains("re_path("))
    {
        return Vec::new();
    }

    let handlers = PythonHandlerIndex::new(by_file);
    let mut out = django_urlconf_routes_with_handlers(text, &handlers);
    dedup_candidates(&mut out);
    out
}

fn django_urlconf_routes_with_handlers(
    text: &str,
    handlers: &PythonHandlerIndex<'_>,
) -> Vec<RouteCandidate> {
    if !is_django_urlconf_text(text) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in find_call_args(text, "path") {
        if let Some(candidate) = django_route_candidate(call.args, false, handlers) {
            out.push(candidate);
        }
    }
    for call in find_call_args(text, "re_path") {
        if let Some(candidate) = django_route_candidate(call.args, true, handlers) {
            out.push(candidate);
        }
    }
    out
}

fn is_django_urlconf_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".py") && (lower.ends_with("urls.py") || lower.contains("/urls/"))
}

fn django_urlconf_files(repo: &Path, project_sources: &ProjectSourceSet) -> Vec<String> {
    let mut out: Vec<String> = if project_sources.has_git_listing() {
        project_sources
            .iter()
            .filter(|path| is_django_urlconf_path(path))
            .map(str::to_string)
            .collect()
    } else {
        let mut files = Vec::new();
        collect_django_urlconf_files(repo, repo, project_sources, &mut files);
        files
    };
    out.sort();
    out.dedup();
    out
}

fn collect_django_urlconf_files(
    repo: &Path,
    dir: &Path,
    project_sources: &ProjectSourceSet,
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if is_urlconf_discovery_skip_dir(name) {
                continue;
            }
            collect_django_urlconf_files(repo, &path, project_sources, out);
        } else if file_type.is_file() {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if is_django_urlconf_path(&rel) && project_sources.contains_project_file(repo, &rel) {
                out.push(rel);
            }
        }
    }
}

fn is_urlconf_discovery_skip_dir(name: &str) -> bool {
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

fn is_django_urlconf_text(text: &str) -> bool {
    text.contains("urlpatterns")
}

fn django_route_candidate(
    args: &str,
    regex: bool,
    handlers: &PythonHandlerIndex,
) -> Option<RouteCandidate> {
    let parts = split_top_level_commas(args);
    let route_literal = string_arg(parts.first()?.trim())?;
    let route = if regex {
        normalize_django_regex_route(&route_literal)
    } else {
        normalize_django_path_route(&route_literal)
    };
    let handler_expr = parts.get(1).and_then(|part| clean_handler_expr(part));
    let handler = handler_expr.and_then(|expr| handlers.find(expr));
    Some(RouteCandidate {
        route,
        method: None,
        handler_id: handler.map(|node| node.aka_id.clone()),
        handler_name: handler.map(|node| node.display_name().to_string()),
    })
}

fn string_arg(arg: &str) -> Option<String> {
    let idx = arg.find(['\'', '"'])?;
    read_string_literal(arg, idx).map(|(literal, _)| literal)
}

fn clean_handler_expr(arg: &str) -> Option<&str> {
    let expr = arg.trim();
    if expr.is_empty() || expr.contains('=') {
        return None;
    }
    let expr = expr.strip_suffix(".as_view()").unwrap_or(expr);
    let expr = expr.strip_suffix(".as_view").unwrap_or(expr);
    let expr = expr.trim();
    if expr
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
    {
        Some(expr)
    } else {
        None
    }
}

fn normalize_django_path_route(value: &str) -> String {
    let mut out = String::new();
    let mut idx = 0usize;
    while let Some(rel_start) = value[idx..].find('<') {
        let start = idx + rel_start;
        out.push_str(&value[idx..start]);
        let Some(rel_end) = value[start..].find('>') else {
            idx = start;
            break;
        };
        let end = start + rel_end;
        let token = &value[start + 1..end];
        let name = token.rsplit(':').next().unwrap_or(token).trim();
        if is_ident_like(name) {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        idx = end + 1;
    }
    out.push_str(&value[idx..]);
    normalize_route_literal_like(&out)
}

fn normalize_django_regex_route(value: &str) -> String {
    let mut route = value
        .trim_start_matches('^')
        .trim_end_matches('$')
        .replace("\\/", "/");
    route = replace_named_regex_groups(&route);
    route = replace_simple_regex_groups(&route);
    route = strip_regex_tokens(&route);
    normalize_route_literal_like(&route)
}

fn replace_named_regex_groups(value: &str) -> String {
    let mut out = String::new();
    let mut idx = 0usize;
    while let Some(rel_start) = value[idx..].find("(?P<") {
        let start = idx + rel_start;
        out.push_str(&value[idx..start]);
        let name_start = start + "(?P<".len();
        let Some(rel_name_end) = value[name_start..].find('>') else {
            idx = start + 1;
            continue;
        };
        let name_end = name_start + rel_name_end;
        let name = &value[name_start..name_end];
        let group_start = name_end + 1;
        let Some(rel_group_end) = value[group_start..].find(')') else {
            idx = group_start;
            continue;
        };
        let group_end = group_start + rel_group_end;
        if is_ident_like(name) {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        idx = group_end + 1;
    }
    out.push_str(&value[idx..]);
    out
}

fn replace_simple_regex_groups(value: &str) -> String {
    let mut out = String::new();
    let mut idx = 0usize;
    let mut next_param = 1usize;
    while let Some(rel_start) = value[idx..].find('(') {
        let start = idx + rel_start;
        if value[start..].starts_with("(?:") || value[start..].starts_with("(?") {
            out.push_str(&value[idx..start + 1]);
            idx = start + 1;
            continue;
        }
        out.push_str(&value[idx..start]);
        let Some(rel_end) = value[start..].find(')') else {
            idx = start;
            break;
        };
        out.push_str(&format!("{{param{next_param}}}"));
        next_param += 1;
        idx = start + rel_end + 1;
    }
    out.push_str(&value[idx..]);
    out
}

fn strip_regex_tokens(value: &str) -> String {
    let mut out = String::new();
    let mut escape = false;
    let mut idx = 0usize;
    while idx < value.len() {
        let ch = value[idx..].chars().next().unwrap_or_default();
        if escape {
            match ch {
                'd' | 'w' | 's' | 'D' | 'W' | 'S' | 'A' | 'Z' | 'b' | 'B' => {}
                _ => out.push(ch),
            }
            escape = false;
            idx += ch.len_utf8();
            continue;
        }
        if ch == '\\' {
            escape = true;
            idx += ch.len_utf8();
            continue;
        }
        if ch == '{' {
            if let Some(rel_end) = value[idx..].find('}') {
                let end = idx + rel_end;
                let name = &value[idx + 1..end];
                if is_ident_like(name) {
                    out.push_str(&value[idx..=end]);
                }
                idx = end + 1;
                continue;
            }
            idx += ch.len_utf8();
            continue;
        }
        if matches!(ch, '?' | '+' | '*' | '[' | ']' | '}' | '|') {
            idx += ch.len_utf8();
            continue;
        }
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn normalize_route_literal_like(value: &str) -> String {
    let mut route = value.trim().trim_matches('/').to_string();
    while route.contains("//") {
        route = route.replace("//", "/");
    }
    if route.is_empty() {
        "/".into()
    } else {
        format!("/{route}")
    }
}

fn is_ident_like(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn dedup_candidates(candidates: &mut Vec<RouteCandidate>) {
    candidates.sort_by(|a, b| {
        a.route
            .cmp(&b.route)
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    candidates.dedup_by(|a, b| a.route == b.route && a.handler_id == b.handler_id);
}

struct PythonHandlerIndex<'a> {
    by_module_member: HashMap<String, &'a SynthNode>,
    by_member: HashMap<String, &'a SynthNode>,
}

impl<'a> PythonHandlerIndex<'a> {
    fn new(by_file: &'a BTreeMap<String, Vec<&'a SynthNode>>) -> Self {
        let mut by_module_member = HashMap::new();
        let mut by_member = HashMap::new();
        for (file_path, nodes) in by_file {
            if !file_path.to_ascii_lowercase().ends_with(".py") {
                continue;
            }
            let module_keys = python_module_keys(file_path);
            for node in nodes {
                if !matches!(node.label.as_str(), "Function" | "Method" | "Class") {
                    continue;
                }
                for key in node_handler_keys(node) {
                    by_member.entry(key.clone()).or_insert(*node);
                    for module in &module_keys {
                        by_module_member
                            .entry(format!("{module}.{key}"))
                            .or_insert(*node);
                    }
                }
            }
        }
        Self {
            by_module_member,
            by_member,
        }
    }

    fn find(&self, expr: &str) -> Option<&'a SynthNode> {
        self.by_module_member
            .get(expr)
            .copied()
            .or_else(|| self.by_member.get(expr).copied())
            .or_else(|| {
                expr.rsplit_once('.')
                    .and_then(|(_, member)| self.by_member.get(member).copied())
            })
    }
}

fn python_module_keys(file_path: &str) -> BTreeSet<String> {
    let normalized = file_path.replace('\\', "/");
    let no_ext = normalized.strip_suffix(".py").unwrap_or(&normalized);
    let dotted = no_ext.replace('/', ".");
    let mut keys = BTreeSet::new();
    keys.insert(dotted.clone());
    if let Some((_, short)) = dotted.rsplit_once('.') {
        keys.insert(short.to_string());
    }
    if let Some((parent, stem)) = dotted.rsplit_once('.') {
        if stem == "views" {
            keys.insert(stem.to_string());
            if let Some((_, app)) = parent.rsplit_once('.') {
                keys.insert(format!("{app}.{stem}"));
            }
        }
    }
    keys
}

fn node_handler_keys(node: &SynthNode) -> Vec<String> {
    let mut keys = BTreeSet::new();
    keys.insert(node.name.clone());
    keys.insert(node.qn.clone());
    let stripped = strip_node_prefix(&node.qn);
    keys.insert(stripped.to_string());
    if let Some((_, member)) = stripped.rsplit_once('.') {
        keys.insert(member.to_string());
    }
    keys.into_iter().collect()
}

fn strip_node_prefix(value: &str) -> &str {
    if let Some(rest) = value.strip_prefix("cbm:") {
        let mut parts = rest.splitn(2, ':');
        if let (Some(_), Some(qn)) = (parts.next(), parts.next()) {
            return qn;
        }
    }
    value
}
