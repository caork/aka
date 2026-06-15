use super::{line_number_at_offset, normalize_route_literal, stable_hash, SynthNode};

pub(super) fn java_source_symbols(file_path: &str, text: &str) -> Vec<SynthNode> {
    let package = java_package_name(text);
    let mut out = Vec::new();
    let mut classes = java_type_declarations(text);
    classes.sort_by_key(|class| class.start);
    for class in classes {
        let class_qn = java_qn(package.as_deref(), None, &class.name);
        out.push(source_node(SourceNodeSpec {
            label: class.label,
            name: class.name.clone(),
            qn: class_qn.clone(),
            file_path,
            start: class.start,
            end: class.end,
            parent_class: None,
            text,
        }));
        for method in java_method_declarations(text, &class) {
            let qn = if method.name == class.name {
                format!("{class_qn}.<init>")
            } else {
                java_qn(None, Some(&class_qn), &method.name)
            };
            out.push(source_node(SourceNodeSpec {
                label: "Method",
                name: method.name,
                qn,
                file_path,
                start: method.start,
                end: method.end,
                parent_class: Some(class_qn.clone()),
                text,
            }));
        }
    }
    out
}

struct SourceNodeSpec<'a> {
    label: &'a str,
    name: String,
    qn: String,
    file_path: &'a str,
    start: usize,
    end: usize,
    parent_class: Option<String>,
    text: &'a str,
}

fn source_node(spec: SourceNodeSpec<'_>) -> SynthNode {
    let start_line = line_number_at_offset(spec.text, spec.start);
    let end_line = line_number_at_offset(spec.text, spec.end);
    let key = format!(
        "{}|{}|{}|{}",
        spec.label, spec.file_path, spec.qn, start_line
    );
    let decorators = java_annotations_before(spec.text, spec.start);
    let (route_path, route_method) = spring_route_hint(&decorators);
    SynthNode {
        aka_id: format!("source:java:{:016x}", stable_hash(&key)),
        qn: spec.qn,
        label: spec.label.into(),
        name: spec.name,
        file_path: spec.file_path.into(),
        start_line,
        end_line,
        language: "java".into(),
        route_path,
        route_method,
        decorators,
        parent_class: spec.parent_class,
        is_exported: false,
        ast_framework_multiplier: 1.0,
        ast_framework_reason: None,
    }
}

fn spring_route_hint(decorators: &[String]) -> (Option<String>, Option<String>) {
    for decorator in decorators {
        let Some(name_end) = decorator
            .find('(')
            .or_else(|| decorator.find(char::is_whitespace))
        else {
            continue;
        };
        let name = decorator[..name_end].trim_start_matches('@');
        let simple = name.rsplit('.').next().unwrap_or(name);
        let method = match simple {
            "GetMapping" => Some("GET"),
            "PostMapping" => Some("POST"),
            "PutMapping" => Some("PUT"),
            "DeleteMapping" => Some("DELETE"),
            "PatchMapping" => Some("PATCH"),
            "RequestMapping" => request_mapping_method(decorator),
            _ => continue,
        };
        return (spring_mapping_path(decorator), method.map(str::to_string));
    }
    (None, None)
}

fn request_mapping_method(decorator: &str) -> Option<&'static str> {
    if decorator.contains("RequestMethod.GET") {
        Some("GET")
    } else if decorator.contains("RequestMethod.POST") {
        Some("POST")
    } else if decorator.contains("RequestMethod.PUT") {
        Some("PUT")
    } else if decorator.contains("RequestMethod.DELETE") {
        Some("DELETE")
    } else if decorator.contains("RequestMethod.PATCH") {
        Some("PATCH")
    } else {
        None
    }
}

fn spring_mapping_path(decorator: &str) -> Option<String> {
    let name_end = decorator.find('(')?;
    let args_start = name_end + 1;
    let args_end = decorator.rfind(')').unwrap_or(decorator.len());
    if args_start >= args_end {
        return Some("/".into());
    }
    let args = &decorator[args_start..args_end];
    first_route_literal(args)
        .map(|path| normalize_route_literal(&path))
        .or_else(|| Some("/".into()))
}

fn first_route_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"') {
            let quote = byte;
            let start = idx + 1;
            let mut end = start;
            let mut escape = false;
            while end < text.len() {
                let current = text.as_bytes()[end];
                if escape {
                    escape = false;
                } else if current == b'\\' {
                    escape = true;
                } else if current == quote {
                    let value = text[start..end].to_string();
                    if value.starts_with('/') {
                        return Some(value);
                    }
                    break;
                }
                end += 1;
            }
            idx = end + 1;
            continue;
        }
        idx += 1;
    }
    None
}

#[derive(Debug, Clone)]
struct JavaTypeDecl {
    label: &'static str,
    name: String,
    start: usize,
    body_start: usize,
    end: usize,
}

fn java_type_declarations(text: &str) -> Vec<JavaTypeDecl> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < text.len() {
        let Some((keyword_pos, keyword, label)) = next_type_keyword(text, offset) else {
            break;
        };
        let name_start = skip_java_ws_and_modifiers(text, keyword_pos + keyword.len());
        let Some((name, _)) = read_java_identifier(text, name_start) else {
            offset = keyword_pos + keyword.len();
            continue;
        };
        let Some(open_rel) = text[keyword_pos..].find('{') else {
            break;
        };
        let body_start = keyword_pos + open_rel;
        let Some(end) = find_matching_brace(text, body_start) else {
            offset = body_start + 1;
            continue;
        };
        out.push(JavaTypeDecl {
            label,
            name: name.to_string(),
            start: keyword_pos,
            body_start,
            end,
        });
        offset = body_start + 1;
    }
    out
}

fn next_type_keyword(text: &str, offset: usize) -> Option<(usize, &'static str, &'static str)> {
    ["class", "interface", "record", "enum"]
        .into_iter()
        .filter_map(|keyword| {
            text[offset..].find(keyword).and_then(|rel| {
                let pos = offset + rel;
                keyword_boundary_ok(text, pos, keyword).then_some((
                    pos,
                    keyword,
                    if keyword == "interface" {
                        "Interface"
                    } else {
                        "Class"
                    },
                ))
            })
        })
        .min_by_key(|(pos, _, _)| *pos)
}

#[derive(Debug, Clone)]
struct JavaMethodDecl {
    name: String,
    start: usize,
    end: usize,
}

fn java_method_declarations(text: &str, class: &JavaTypeDecl) -> Vec<JavaMethodDecl> {
    let mut out = Vec::new();
    let mut cursor = class.body_start + 1;
    while cursor < class.end {
        let Some(open_rel) = text[cursor..class.end].find('(') else {
            break;
        };
        let open = cursor + open_rel;
        let Some((name, name_start)) = identifier_before(text, open) else {
            cursor = open + 1;
            continue;
        };
        if !is_method_name_candidate(&name) {
            cursor = open + 1;
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            cursor = open + 1;
            continue;
        };
        let after = skip_java_ws(text, close + 1);
        let Some(body_start) = method_body_start(text, after, class.end) else {
            cursor = close + 1;
            continue;
        };
        let Some(end) = find_matching_brace(text, body_start) else {
            cursor = body_start + 1;
            continue;
        };
        if is_probable_method_declaration(text, class, name_start, open) {
            out.push(JavaMethodDecl {
                name,
                start: declaration_start(text, name_start),
                end,
            });
            cursor = end + 1;
        } else {
            cursor = close + 1;
        }
    }
    out
}

fn method_body_start(text: &str, mut pos: usize, limit: usize) -> Option<usize> {
    while pos < limit {
        match text.as_bytes().get(pos).copied() {
            Some(b'{') => return Some(pos),
            Some(b'\n' | b';' | b'=') => return None,
            Some(_) => pos += 1,
            None => return None,
        }
    }
    None
}

fn is_probable_method_declaration(
    text: &str,
    class: &JavaTypeDecl,
    name_start: usize,
    open: usize,
) -> bool {
    if type_depth_at(text, class, name_start) != 0 {
        return false;
    }
    let prefix_start = text[..name_start]
        .rfind(['\n', ';', '{', '}'])
        .map(|idx| idx + 1)
        .unwrap_or(class.body_start + 1);
    let prefix = text[prefix_start..name_start].trim();
    if prefix.is_empty() || prefix.starts_with('@') || prefix.contains('=') || prefix.contains("->")
    {
        return false;
    }
    let before_open = text[prefix_start..open].trim();
    !before_open.starts_with("new ")
}

fn type_depth_at(text: &str, class: &JavaTypeDecl, pos: usize) -> i32 {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for byte in text[class.body_start + 1..pos.min(class.end)].bytes() {
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
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn declaration_start(text: &str, name_start: usize) -> usize {
    let mut start = text[..name_start]
        .rfind(['\n', ';', '{', '}'])
        .map(|idx| idx + 1)
        .unwrap_or(0);
    while let Some(annotation_start) = previous_annotation_start(text, start) {
        start = annotation_start;
    }
    start
}

fn previous_annotation_start(text: &str, start: usize) -> Option<usize> {
    let prefix = &text[..start];
    let line_start = prefix
        .trim_end_matches([' ', '\t', '\r', '\n'])
        .rfind('\n')?
        + 1;
    let line = text[line_start..start].trim();
    line.starts_with('@').then_some(line_start)
}

fn java_annotations_before(text: &str, start: usize) -> Vec<String> {
    let mut annotations = Vec::new();
    let mut cursor = start;
    while let Some((line, next)) = line_at(text, cursor) {
        let trimmed = line.trim();
        if !trimmed.starts_with('@') {
            break;
        }
        annotations.push(trimmed.to_string());
        cursor = next;
    }
    if !annotations.is_empty() {
        return annotations;
    }

    let mut cursor = start;
    while let Some(line_start) = previous_nonblank_line_start(text, cursor) {
        let line_end = text[line_start..cursor]
            .find('\n')
            .map(|rel| line_start + rel)
            .unwrap_or(cursor);
        let line = text[line_start..line_end].trim();
        if !line.starts_with('@') {
            break;
        }
        annotations.push(line.to_string());
        cursor = line_start;
    }
    annotations.reverse();
    annotations
}

fn line_at(text: &str, start: usize) -> Option<(&str, usize)> {
    if start >= text.len() {
        return None;
    }
    let line_end = text[start..]
        .find('\n')
        .map(|rel| start + rel)
        .unwrap_or(text.len());
    Some((&text[start..line_end], (line_end + 1).min(text.len())))
}

fn previous_nonblank_line_start(text: &str, cursor: usize) -> Option<usize> {
    let prefix = &text[..cursor.min(text.len())];
    let mut end = prefix.trim_end_matches([' ', '\t', '\r', '\n']).len();
    if end == 0 {
        return None;
    }
    while end > 0 {
        let start = text[..end].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        if !text[start..end].trim().is_empty() {
            return Some(start);
        }
        end = start.saturating_sub(1);
    }
    None
}

fn identifier_before(text: &str, pos: usize) -> Option<(String, usize)> {
    let mut end = pos;
    while end > 0
        && text
            .as_bytes()
            .get(end - 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if !is_java_ident_continue(ch) {
            break;
        }
        start -= ch.len_utf8();
    }
    (start < end).then(|| (text[start..end].to_string(), start))
}

fn is_method_name_candidate(name: &str) -> bool {
    !matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "try"
            | "return"
            | "throw"
            | "new"
            | "super"
            | "this"
    )
}

fn java_package_name(text: &str) -> Option<String> {
    for line in text.lines().take(80) {
        let trimmed = line.trim();
        if !trimmed.starts_with("package ") {
            continue;
        }
        return trimmed
            .trim_start_matches("package ")
            .trim_end_matches(';')
            .split_whitespace()
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    None
}

fn java_qn(package: Option<&str>, owner: Option<&str>, name: &str) -> String {
    let mut parts = Vec::new();
    if let Some(package) = package.filter(|value| !value.is_empty()) {
        parts.push(package);
    }
    if let Some(owner) = owner.filter(|value| !value.is_empty()) {
        parts.push(owner);
    }
    parts.push(name);
    parts.join(".")
}

fn skip_java_ws_and_modifiers(text: &str, mut idx: usize) -> usize {
    loop {
        idx = skip_java_ws(text, idx);
        let Some((word, end)) = read_java_identifier(text, idx) else {
            return idx;
        };
        if matches!(
            word,
            "public"
                | "protected"
                | "private"
                | "abstract"
                | "final"
                | "sealed"
                | "static"
                | "strictfp"
        ) {
            idx = end;
            continue;
        }
        return idx;
    }
}

fn skip_java_ws(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn read_java_identifier(text: &str, start: usize) -> Option<(&str, usize)> {
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_java_ident_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (rel, ch) in chars {
        if !is_java_ident_continue(ch) {
            break;
        }
        end = start + rel + ch.len_utf8();
    }
    Some((&text[start..end], end))
}

fn is_java_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

fn is_java_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

fn keyword_boundary_ok(text: &str, start: usize, keyword: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + keyword.len())
        .copied()
        .map(char::from);
    before.is_none_or(|ch| !is_java_ident_continue(ch))
        && after.is_none_or(|ch| !is_java_ident_continue(ch))
}

fn find_matching_paren(text: &str, open: usize) -> Option<usize> {
    find_matching_pair(text, open, b'(', b')')
}

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    find_matching_pair(text, open, b'{', b'}')
}

fn find_matching_pair(text: &str, open: usize, left: u8, right: u8) -> Option<usize> {
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
            b if b == left => depth += 1,
            b if b == right => {
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
