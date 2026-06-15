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
    let module_to_file: HashMap<String, String> = file_paths
        .iter()
        .map(|file_path| (python_file_module_name(file_path), file_path.clone()))
        .collect();
    let router_export_to_file = router_export_to_file(&file_paths);
    let mut include_edges: BTreeMap<String, Vec<DjangoUrlIncludeEdge>> = BTreeMap::new();
    for file_path in &file_paths {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let imported_routers = imported_router_modules(&text, &router_export_to_file);
        for include in django_urlconf_includes(&text, &imported_routers) {
            if let Some(target_file) = module_to_file.get(&include.module) {
                include_edges
                    .entry(file_path.clone())
                    .or_default()
                    .push(DjangoUrlIncludeEdge {
                        target_file: target_file.clone(),
                        prefix: include.prefix,
                    });
            }
        }
    }
    let include_prefixes = transitive_include_prefixes(&file_paths, &include_edges);

    for file_path in file_paths {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let prefixes = include_prefixes.get(&file_path).cloned().unwrap_or_default();
        let mut candidates = django_urlconf_routes_with_handlers(&text, &handlers);
        candidates.extend(drf_router_routes_with_handlers(&text, &handlers));
        if !prefixes.is_empty() {
            candidates = candidates
                .into_iter()
                .flat_map(|candidate| {
                    prefixes
                        .iter()
                        .map(move |prefix| prefix_route_candidate(prefix, &candidate))
                })
                .collect();
        }
        if candidates.is_empty() {
            continue;
        }
        dedup_candidates(&mut candidates);
        out.insert(file_path.to_string(), candidates);
    }
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

fn drf_router_routes_with_handlers(
    text: &str,
    handlers: &PythonHandlerIndex<'_>,
) -> Vec<RouteCandidate> {
    if !(text.contains("DefaultRouter") || text.contains("SimpleRouter"))
        || !text.contains(".register")
    {
        return Vec::new();
    }
    let routers = drf_router_names(text);
    if routers.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for router in routers {
        let call_name = format!("{router}.register");
        for call in find_call_args(text, &call_name) {
            let Some((prefix, viewset_expr)) = drf_router_register(call.args) else {
                continue;
            };
            let viewset_handler = handlers.find(&viewset_expr);
            let collection_handler = handlers
                .find_method(&viewset_expr, "list")
                .or(viewset_handler);
            let detail_handler = handlers
                .find_method(&viewset_expr, "retrieve")
                .or(viewset_handler);
            let collection = normalize_django_path_route(&prefix);
            out.push(RouteCandidate {
                route: collection.clone(),
                method: None,
                handler_id: collection_handler.map(|node| node.aka_id.clone()),
                handler_name: collection_handler.map(|node| node.display_name().to_string()),
            });
            out.push(RouteCandidate {
                route: join_django_routes(&collection, "{id}"),
                method: None,
                handler_id: detail_handler.map(|node| node.aka_id.clone()),
                handler_name: detail_handler.map(|node| node.display_name().to_string()),
            });
        }
    }
    out
}

fn drf_router_names(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for constructor in ["DefaultRouter", "SimpleRouter"] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(constructor) {
            let start = offset + pos;
            let Some(router_name) = assigned_name_before_call(text, start) else {
                offset = start + constructor.len();
                continue;
            };
            out.insert(router_name);
            offset = start + constructor.len();
        }
    }
    out
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

fn drf_router_register(args: &str) -> Option<(String, String)> {
    let parts = split_top_level_commas(args);
    let prefix = string_arg(parts.first()?.trim())?;
    let viewset = parts.get(1).and_then(|part| clean_handler_expr(part))?;
    Some((prefix, viewset.to_string()))
}

fn router_export_to_file(file_paths: &[String]) -> HashMap<String, String> {
    file_paths
        .iter()
        .map(|file_path| {
            (
                format!("{}.router", python_file_module_name(file_path)),
                file_path.clone(),
            )
        })
        .collect()
}

fn imported_router_modules(
    text: &str,
    router_export_to_file: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imported)) = rest.split_once(" import ") else {
            continue;
        };
        for item in imported.split(',') {
            let item = item.trim();
            let (name, alias) = split_python_import_alias(item);
            let exported = format!("{module}.{name}");
            if let Some(file_path) = router_export_to_file.get(&exported) {
                out.insert(alias.unwrap_or(name).to_string(), file_path.clone());
            }
        }
    }
    out
}

fn split_python_import_alias(item: &str) -> (&str, Option<&str>) {
    if let Some((name, alias)) = item.split_once(" as ") {
        (name.trim(), Some(alias.trim()))
    } else {
        (item.trim(), None)
    }
}

#[derive(Debug)]
struct DjangoUrlInclude {
    prefix: String,
    module: String,
}

#[derive(Debug)]
struct DjangoUrlIncludeEdge {
    target_file: String,
    prefix: String,
}

fn transitive_include_prefixes(
    file_paths: &[String],
    include_edges: &BTreeMap<String, Vec<DjangoUrlIncludeEdge>>,
) -> BTreeMap<String, Vec<String>> {
    let mut has_parent = BTreeSet::new();
    for edges in include_edges.values() {
        for edge in edges {
            has_parent.insert(edge.target_file.clone());
        }
    }
    let mut roots: Vec<String> = file_paths
        .iter()
        .filter(|file_path| !has_parent.contains(*file_path))
        .cloned()
        .collect();
    if roots.is_empty() {
        roots = file_paths.to_vec();
    }

    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for root in roots {
        collect_include_prefixes(&root, "", include_edges, &mut BTreeSet::new(), &mut out);
    }
    for prefixes in out.values_mut() {
        prefixes.sort();
        prefixes.dedup();
    }
    out
}

fn collect_include_prefixes(
    file_path: &str,
    prefix: &str,
    include_edges: &BTreeMap<String, Vec<DjangoUrlIncludeEdge>>,
    stack: &mut BTreeSet<String>,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    if !stack.insert(file_path.to_string()) {
        return;
    }
    if !prefix.is_empty() {
        out.entry(file_path.to_string())
            .or_default()
            .push(prefix.to_string());
    }
    if let Some(edges) = include_edges.get(file_path) {
        for edge in edges {
            let next_prefix = join_django_routes(prefix, &edge.prefix);
            collect_include_prefixes(
                &edge.target_file,
                &next_prefix,
                include_edges,
                stack,
                out,
            );
        }
    }
    stack.remove(file_path);
}

fn django_urlconf_includes(
    text: &str,
    imported_routers: &HashMap<String, String>,
) -> Vec<DjangoUrlInclude> {
    let mut out = Vec::new();
    for call in find_call_args(text, "path") {
        if let Some(include) = django_url_include(call.args, false, imported_routers) {
            out.push(include);
        }
    }
    for call in find_call_args(text, "re_path") {
        if let Some(include) = django_url_include(call.args, true, imported_routers) {
            out.push(include);
        }
    }
    out
}

fn django_url_include(
    args: &str,
    regex: bool,
    imported_routers: &HashMap<String, String>,
) -> Option<DjangoUrlInclude> {
    let parts = split_top_level_commas(args);
    let route_literal = string_arg(parts.first()?.trim())?;
    let prefix = if regex {
        normalize_django_regex_route(&route_literal)
    } else {
        normalize_django_path_route(&route_literal)
    };
    let include_expr = parts.get(1)?.trim();
    if !include_expr.starts_with("include") {
        return None;
    }
    let include_args = call_inner_args(include_expr, "include")?;
    let module = django_include_module(include_args).or_else(|| {
        django_include_router_file(include_args, imported_routers)
            .map(|file_path| python_file_module_name(&file_path))
    })?;
    Some(DjangoUrlInclude { prefix, module })
}

fn django_include_router_file(
    args: &str,
    imported_routers: &HashMap<String, String>,
) -> Option<String> {
    let first = split_top_level_commas(args).into_iter().next()?.trim();
    let router_name = first.strip_suffix(".urls")?.trim();
    imported_routers.get(router_name).cloned()
}

fn call_inner_args<'a>(expr: &'a str, name: &str) -> Option<&'a str> {
    let expr = expr.trim();
    let rest = expr.strip_prefix(name)?;
    let open = rest.find('(')? + name.len();
    let close = expr.rfind(')')?;
    if close <= open {
        return None;
    }
    Some(&expr[open + 1..close])
}

fn django_include_module(args: &str) -> Option<String> {
    let first = split_top_level_commas(args).into_iter().next()?.trim();
    if let Some(module) = string_arg(first) {
        return Some(module);
    }
    if first.starts_with('(') {
        return string_arg(first);
    }
    None
}

fn prefix_route_candidate(prefix: &str, candidate: &RouteCandidate) -> RouteCandidate {
    RouteCandidate {
        route: join_django_routes(prefix, &candidate.route),
        method: candidate.method.clone(),
        handler_id: candidate.handler_id.clone(),
        handler_name: candidate.handler_name.clone(),
    }
}

fn join_django_routes(prefix: &str, route: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let route = route.trim_start_matches('/');
    if prefix.is_empty() || prefix == "/" {
        format!("/{route}").trim_end_matches('/').to_string()
    } else if route.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{route}")
    }
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
    by_owner_method: HashMap<String, &'a SynthNode>,
}

impl<'a> PythonHandlerIndex<'a> {
    fn new(by_file: &'a BTreeMap<String, Vec<&'a SynthNode>>) -> Self {
        let mut by_module_member = HashMap::new();
        let mut by_member = HashMap::new();
        let mut by_owner_method = HashMap::new();
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
                if matches!(node.label.as_str(), "Function" | "Method") {
                    for key in node_method_keys(node) {
                        by_owner_method.entry(key).or_insert(*node);
                    }
                }
            }
        }
        Self {
            by_module_member,
            by_member,
            by_owner_method,
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

    fn find_method(&self, owner_expr: &str, method_name: &str) -> Option<&'a SynthNode> {
        let mut keys = Vec::new();
        keys.push(format!("{owner_expr}.{method_name}"));
        if let Some((_, owner)) = owner_expr.rsplit_once('.') {
            keys.push(format!("{owner}.{method_name}"));
        }
        keys.into_iter()
            .find_map(|key| self.by_owner_method.get(&key).copied())
    }
}

fn python_module_keys(file_path: &str) -> BTreeSet<String> {
    let dotted = python_file_module_name(file_path);
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

fn python_file_module_name(file_path: &str) -> String {
    let normalized = file_path.replace('\\', "/");
    let no_ext = normalized.strip_suffix(".py").unwrap_or(&normalized);
    no_ext.replace('/', ".")
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

fn node_method_keys(node: &SynthNode) -> Vec<String> {
    let mut keys = BTreeSet::new();
    let stripped = strip_node_prefix(&node.qn);
    keys.insert(stripped.to_string());
    if let Some(parent) = node.parent_class.as_ref() {
        let parent = strip_node_prefix(parent);
        keys.insert(format!("{parent}.{}", node.name));
        if let Some((_, short_parent)) = parent.rsplit_once('.') {
            keys.insert(format!("{short_parent}.{}", node.name));
        }
    }
    if let Some((owner, method)) = stripped.rsplit_once('.') {
        keys.insert(format!("{owner}.{method}"));
        if let Some((_, short_owner)) = owner.rsplit_once('.') {
            keys.insert(format!("{short_owner}.{method}"));
        }
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
