use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::{
    find_matching_paren, join_route_paths, normalize_route_literal, read_repo_text,
    read_string_literal, source_scan::is_noisy_source_path, SynthNode,
};

#[derive(Debug, Default)]
pub(super) struct PythonRoutePrefixes {
    pub(super) include: Vec<String>,
    pub(super) local_by_router: HashMap<String, String>,
}

pub(super) fn python_route_prefixes_for_node(
    python_prefixes: Option<&PythonRoutePrefixes>,
    node: &SynthNode,
) -> Vec<String> {
    let Some(prefixes) = python_prefixes else {
        return vec![String::new()];
    };
    let include_prefixes: Vec<String> = if prefixes.include.is_empty() {
        vec![String::new()]
    } else {
        prefixes.include.clone()
    };
    let local_prefix = python_route_local_prefix(prefixes, node);
    include_prefixes
        .into_iter()
        .map(|prefix| {
            if let Some(local) = local_prefix {
                join_route_paths(&prefix, local)
            } else {
                prefix
            }
        })
        .collect()
}

fn python_route_local_prefix<'a>(
    prefixes: &'a PythonRoutePrefixes,
    node: &SynthNode,
) -> Option<&'a str> {
    router_name_from_python_decorators(&node.decorators)
        .and_then(|router| prefixes.local_by_router.get(router))
        .map(String::as_str)
}

pub(super) fn python_router_prefixes_by_file<'a>(
    repo: &Path,
    file_paths: impl Iterator<Item = &'a str>,
) -> BTreeMap<String, PythonRoutePrefixes> {
    let mut python_files: Vec<String> = file_paths
        .filter(|path| path.to_ascii_lowercase().ends_with(".py"))
        .map(str::to_string)
        .collect();
    python_files.extend(repo_python_source_files(repo));
    python_files.sort();
    python_files.dedup();
    if python_files.is_empty() {
        return BTreeMap::new();
    }

    let mut short_to_files: HashMap<String, Vec<String>> = HashMap::new();
    let mut long_to_files: HashMap<String, Vec<String>> = HashMap::new();
    for file_path in &python_files {
        short_to_files
            .entry(python_file_short_key(file_path))
            .or_default()
            .push(file_path.clone());
        if let Some(long_key) = python_file_long_key(file_path) {
            long_to_files
                .entry(long_key)
                .or_default()
                .push(file_path.clone());
        }
        if let Some(package_key) = python_package_file_key(file_path) {
            long_to_files
                .entry(package_key.clone())
                .or_default()
                .push(file_path.clone());
            short_to_files
                .entry(
                    package_key
                        .rsplit('/')
                        .next()
                        .unwrap_or(&package_key)
                        .to_string(),
                )
                .or_default()
                .push(file_path.clone());
        }
    }

    let mut out: BTreeMap<String, PythonRoutePrefixes> = BTreeMap::new();
    let mut include_edges: BTreeMap<String, Vec<PythonIncludeRouterEdge>> = BTreeMap::new();
    for file_path in &python_files {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let local_by_router = extract_python_local_router_prefixes(&text);
        if !local_by_router.is_empty() {
            out.entry(file_path.clone()).or_default().local_by_router = local_by_router;
        }
        let imports = python_router_imports(&text);
        for include in extract_python_router_includes(&text) {
            let targets = python_include_targets(
                &include.router_expr,
                &imports,
                &short_to_files,
                &long_to_files,
            );
            for target in targets {
                include_edges
                    .entry(file_path.clone())
                    .or_default()
                    .push(PythonIncludeRouterEdge {
                        target_file: target.clone(),
                        prefix: normalize_route_literal(&include.prefix),
                    });
            }
        }
    }
    let transitive_prefixes =
        transitive_python_include_prefixes(&python_files, &include_edges, &out);
    for (file_path, prefixes) in transitive_prefixes {
        out.entry(file_path).or_default().include.extend(prefixes);
    }
    for prefixes in out.values_mut() {
        prefixes.include.sort();
        prefixes.include.dedup();
    }
    out
}

fn python_include_targets(
    router_expr: &str,
    imports: &PythonRouterImports,
    short_to_files: &HashMap<String, Vec<String>>,
    long_to_files: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    if let Some((module_expr, _)) = python_router_module_expr(router_expr) {
        let module_name = module_expr.rsplit('.').next().unwrap_or(module_expr);
        return imports
            .module_aliases
            .get(module_name)
            .and_then(|long_key| long_to_files.get(long_key))
            .cloned()
            .or_else(|| short_to_files.get(module_name).cloned())
            .unwrap_or_default();
    }
    if let Some(import) = imports.router_names.get(router_expr) {
        if let Some(long_key) = &import.long_key {
            return long_to_files.get(long_key).cloned().unwrap_or_default();
        }
        return short_to_files
            .get(&import.short_key)
            .cloned()
            .unwrap_or_default();
    }
    Vec::new()
}

fn repo_python_source_files(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_repo_python_source_files(repo, repo, &mut out);
    out
}

fn collect_repo_python_source_files(repo: &Path, dir: &Path, out: &mut Vec<String>) {
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
            let rel = path
                .strip_prefix(repo)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| name.to_string());
            if is_noisy_source_path(&rel) {
                continue;
            }
            collect_repo_python_source_files(repo, &path, out);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
        {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[derive(Debug)]
struct PythonIncludeRouter {
    router_expr: String,
    prefix: String,
}

#[derive(Debug)]
struct PythonIncludeRouterEdge {
    target_file: String,
    prefix: String,
}

#[derive(Debug)]
struct PythonRouterImport {
    short_key: String,
    long_key: Option<String>,
}

#[derive(Debug, Default)]
struct PythonRouterImports {
    router_names: HashMap<String, PythonRouterImport>,
    module_aliases: HashMap<String, String>,
}

fn python_router_imports(text: &str) -> PythonRouterImports {
    let mut imports = PythonRouterImports::default();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imported)) = rest.split_once(" import ") else {
            continue;
        };
        for item in imported.split(',') {
            let item = item.trim();
            if item.is_empty() || item == "*" {
                continue;
            }
            let (name, alias) = split_python_import_alias(item);
            if is_python_router_export_name(name) {
                let local = alias.unwrap_or(name);
                let short_key = module
                    .trim_start_matches('.')
                    .rsplit('.')
                    .next()
                    .unwrap_or(module)
                    .to_string();
                imports.router_names.insert(
                    local.to_string(),
                    PythonRouterImport {
                        short_key,
                        long_key: python_module_long_key(module),
                    },
                );
            } else if let Some(long_key) = python_module_long_key(&format!("{module}.{name}")) {
                imports
                    .module_aliases
                    .insert(alias.unwrap_or(name).to_string(), long_key);
            }
        }
    }
    imports
}

fn is_python_router_export_name(name: &str) -> bool {
    matches!(name, "router" | "bp" | "blueprint")
}

fn python_router_module_expr(expr: &str) -> Option<(&str, &str)> {
    let (module_expr, export_name) = expr.rsplit_once('.')?;
    is_python_router_export_name(export_name).then_some((module_expr, export_name))
}

fn extract_python_local_router_prefixes(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    extend_python_constructor_prefixes(text, "APIRouter", "prefix", &mut out);
    extend_python_constructor_prefixes(text, "Blueprint", "url_prefix", &mut out);
    out
}

fn extend_python_constructor_prefixes(
    text: &str,
    constructor: &str,
    prefix_kw: &str,
    out: &mut HashMap<String, String>,
) {
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(constructor) {
        let name_start = offset + pos;
        let name_end = name_start + constructor.len();
        let Some(open_rel) = text[name_start..].find('(') else {
            break;
        };
        let open = name_start + open_rel;
        if !text[name_end..open].trim().is_empty() {
            offset = name_end;
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        if let Some(router_name) = assigned_name_before_call(text, name_start) {
            let args = &text[open + 1..close];
            if let Some(prefix) = keyword_string_arg(args, prefix_kw) {
                out.insert(router_name, normalize_route_literal(&prefix));
            }
        }
        offset = close + 1;
    }
}

fn assigned_name_before_call(text: &str, call_start: usize) -> Option<String> {
    let line_start = text[..call_start].rfind('\n').map_or(0, |idx| idx + 1);
    let before = text[line_start..call_start].trim();
    let lhs = before.split_once('=')?.0.trim();
    if lhs.contains(' ') || lhs.contains('.') || lhs.is_empty() {
        return None;
    }
    Some(lhs.to_string()).filter(|name| name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()))
}

fn router_name_from_python_decorators(decorators: &[String]) -> Option<&str> {
    decorators.iter().find_map(|decorator| {
        let text = decorator.trim().trim_start_matches('@');
        let (receiver, method) = text.split_once('.')?;
        let method = method
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or(method);
        if matches!(
            method,
            "get"
                | "post"
                | "put"
                | "patch"
                | "delete"
                | "head"
                | "options"
                | "api_route"
                | "route"
                | "websocket"
                | "websocket_route"
        ) {
            Some(receiver)
        } else {
            None
        }
    })
}

fn split_python_import_alias(item: &str) -> (&str, Option<&str>) {
    if let Some((name, alias)) = item.split_once(" as ") {
        (name.trim(), Some(alias.trim()))
    } else {
        (item.trim(), None)
    }
}

fn extract_python_router_includes(text: &str) -> Vec<PythonIncludeRouter> {
    let mut out = Vec::new();
    if text.contains("include_router") {
        out.extend(extract_python_router_mounts(
            text,
            ".include_router",
            "prefix",
        ));
    }
    if text.contains("register_blueprint") {
        out.extend(extract_python_router_mounts(
            text,
            ".register_blueprint",
            "url_prefix",
        ));
    }
    out
}

fn transitive_python_include_prefixes(
    file_paths: &[String],
    include_edges: &BTreeMap<String, Vec<PythonIncludeRouterEdge>>,
    prefixes_by_file: &BTreeMap<String, PythonRoutePrefixes>,
) -> BTreeMap<String, Vec<String>> {
    let mut has_parent = BTreeSet::new();
    for edges in include_edges.values() {
        for edge in edges {
            has_parent.insert(edge.target_file.clone());
        }
    }
    let roots: Vec<String> = file_paths
        .iter()
        .filter(|file_path| !has_parent.contains(*file_path))
        .cloned()
        .collect();
    let mut out = BTreeMap::new();
    for root in roots {
        collect_python_include_prefixes(
            &root,
            "",
            include_edges,
            prefixes_by_file,
            &mut BTreeSet::new(),
            &mut out,
        );
    }
    out
}

fn collect_python_include_prefixes(
    file_path: &str,
    prefix: &str,
    include_edges: &BTreeMap<String, Vec<PythonIncludeRouterEdge>>,
    prefixes_by_file: &BTreeMap<String, PythonRoutePrefixes>,
    stack: &mut BTreeSet<String>,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    if !stack.insert(file_path.to_string()) {
        return;
    }
    if let Some(edges) = include_edges.get(file_path) {
        for edge in edges {
            let next_prefix = join_route_paths(prefix, &edge.prefix);
            out.entry(edge.target_file.clone())
                .or_default()
                .push(next_prefix.clone());
            let child_prefix = python_file_router_prefix(prefixes_by_file, &edge.target_file)
                .map(|local| join_route_paths(&next_prefix, local))
                .unwrap_or_else(|| next_prefix.clone());
            collect_python_include_prefixes(
                &edge.target_file,
                &child_prefix,
                include_edges,
                prefixes_by_file,
                stack,
                out,
            );
        }
    }
    stack.remove(file_path);
}

fn python_file_router_prefix<'a>(
    prefixes_by_file: &'a BTreeMap<String, PythonRoutePrefixes>,
    file_path: &str,
) -> Option<&'a str> {
    let prefixes = prefixes_by_file.get(file_path)?;
    prefixes
        .local_by_router
        .get("router")
        .or_else(|| {
            (prefixes.local_by_router.len() == 1)
                .then(|| prefixes.local_by_router.values().next())
                .flatten()
        })
        .map(String::as_str)
}

fn extract_python_router_mounts(
    text: &str,
    call_name: &str,
    prefix_kw: &str,
) -> Vec<PythonIncludeRouter> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(call_name) {
        let call_start = offset + pos;
        let Some(open_rel) = text[call_start..].find('(') else {
            break;
        };
        let open = call_start + open_rel;
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        let args = &text[open + 1..close];
        if let Some(router_expr) = first_call_argument(args) {
            out.push(PythonIncludeRouter {
                router_expr,
                prefix: keyword_string_arg(args, prefix_kw).unwrap_or_else(|| "/".into()),
            });
        }
        offset = close + 1;
    }
    out
}

fn first_call_argument(args: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
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
            b',' if depth == 0 => return clean_python_expr(&args[..idx]),
            _ => {}
        }
    }
    clean_python_expr(args)
}

fn clean_python_expr(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() || expr.contains('=') {
        None
    } else {
        Some(expr.to_string())
    }
}

fn keyword_string_arg(args: &str, keyword: &str) -> Option<String> {
    let needle = format!("{keyword}=");
    let compact = args.replace(' ', "");
    let pos = compact.find(&needle)?;
    let start = pos + needle.len();
    read_string_literal(&compact, start).map(|(literal, _)| literal)
}

fn python_file_short_key(rel: &str) -> String {
    let normalized = rel.replace('\\', "/");
    let file = normalized.rsplit('/').next().unwrap_or(&normalized);
    file.strip_suffix(".py").unwrap_or(file).to_string()
}

fn python_file_long_key(rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    let no_ext = normalized.strip_suffix(".py").unwrap_or(&normalized);
    let (parent_path, stem) = no_ext.rsplit_once('/')?;
    let parent = parent_path.rsplit('/').next().unwrap_or(parent_path);
    Some(format!("{parent}/{stem}"))
}

fn python_package_file_key(rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    normalized
        .strip_suffix("/__init__.py")
        .filter(|package| !package.is_empty())
        .map(str::to_string)
}

fn python_module_long_key(module: &str) -> Option<String> {
    let stripped = module.trim_start_matches('.');
    let (parent_path, stem) = stripped.rsplit_once('.')?;
    let parent = parent_path.rsplit('.').next().unwrap_or(parent_path);
    Some(format!("{parent}/{stem}"))
}
