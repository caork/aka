use std::collections::BTreeSet;

use super::{find_matching_paren, split_top_level_commas};

pub(super) fn java_column_names(body: &str) -> Vec<String> {
    java_annotation_arg_strings(body, &["Column", "JoinColumn"], &["name", "value"])
}

pub(super) fn java_relationship_targets(body: &str) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for statement in body.split(';') {
        if !java_has_annotation_named(
            statement,
            &["ManyToOne", "OneToOne", "OneToMany", "ManyToMany"],
        ) {
            continue;
        }
        for target in java_field_type_candidates(statement) {
            targets.insert(target);
        }
    }
    targets.into_iter().collect()
}

fn java_field_type_candidates(statement: &str) -> Vec<String> {
    let mut text = strip_java_annotations(statement);
    if let Some((before_init, _)) = text.split_once('=') {
        text = before_init.to_string();
    }
    if let Some(pos) = text.rfind('{') {
        text = text[pos + 1..].to_string();
    }
    if let Some(pos) = text.rfind('}') {
        text = text[pos + 1..].to_string();
    }
    let mut parts: Vec<&str> = text.split_whitespace().collect();
    while parts
        .first()
        .is_some_and(|part| java_field_modifier(part.trim()))
    {
        parts.remove(0);
    }
    if parts.len() < 2 {
        return Vec::new();
    }
    let type_text = parts[..parts.len() - 1].join(" ");
    let mut candidates = BTreeSet::new();
    if let Some(generic) = generic_type_args(&type_text) {
        for arg in split_top_level_commas(generic) {
            let arg = arg
                .trim()
                .trim_start_matches("? extends ")
                .trim_start_matches("? super ")
                .trim();
            if let Some(target) = first_java_type_name(arg) {
                candidates.insert(target);
            }
        }
    } else if let Some(target) = first_java_type_name(&type_text) {
        candidates.insert(target);
    }
    candidates.into_iter().collect()
}

fn java_field_modifier(part: &str) -> bool {
    matches!(
        part,
        "private" | "protected" | "public" | "static" | "final" | "transient" | "volatile" | "var"
    )
}

fn first_java_type_name(text: &str) -> Option<String> {
    let text = text
        .trim()
        .trim_end_matches("[]")
        .trim_matches(['(', ')'])
        .trim();
    let name = text
        .split(['<', '>', '[', ']', ',', ' '])
        .next()
        .unwrap_or("")
        .trim();
    let name = strip_package(name);
    is_type_name(name).then(|| name.to_string())
}

fn generic_type_args(text: &str) -> Option<&str> {
    let open = text.find('<')?;
    let close = matching_angle(text, open).unwrap_or(text.len());
    text.get(open + 1..close)
}

fn matching_angle(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().skip_while(|(idx, _)| *idx < open) {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_java_annotations(statement: &str) -> String {
    let mut out = String::new();
    let mut idx = 0usize;
    while idx < statement.len() {
        if statement.as_bytes().get(idx) == Some(&b'@') {
            let Some((_, mut end)) = java_annotation_name_at(statement, idx) else {
                out.push('@');
                idx += 1;
                continue;
            };
            while statement
                .as_bytes()
                .get(end)
                .is_some_and(u8::is_ascii_whitespace)
            {
                end += 1;
            }
            if statement.as_bytes().get(end) == Some(&b'(') {
                end =
                    find_matching_paren(statement, end).map_or(statement.len(), |close| close + 1);
            }
            idx = end;
            continue;
        }
        let ch = statement[idx..].chars().next().unwrap_or_default();
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn java_has_annotation_named(text: &str, expected: &[&str]) -> bool {
    let mut idx = 0usize;
    while let Some(pos) = text[idx..].find('@') {
        let at = idx + pos;
        if let Some((name, end)) = java_annotation_name_at(text, at) {
            let short = strip_package(&name);
            if expected.contains(&short) {
                return true;
            }
            idx = end;
        } else {
            idx = at + 1;
        }
    }
    false
}

fn java_annotation_arg_strings(text: &str, names: &[&str], keys: &[&str]) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut idx = 0usize;
    while let Some(pos) = text[idx..].find('@') {
        let at = idx + pos;
        let Some((name, mut end)) = java_annotation_name_at(text, at) else {
            idx = at + 1;
            continue;
        };
        let short = strip_package(&name);
        while text
            .as_bytes()
            .get(end)
            .is_some_and(u8::is_ascii_whitespace)
        {
            end += 1;
        }
        if names.contains(&short) && text.as_bytes().get(end) == Some(&b'(') {
            let close = find_matching_paren(text, end).unwrap_or(text.len().saturating_sub(1));
            if let Some(annotation) = text.get(at..=close) {
                if let Some(value) = annotation_arg_string(annotation, keys) {
                    out.insert(value);
                }
            }
            idx = close.saturating_add(1);
        } else {
            idx = end;
        }
    }
    out.into_iter().collect()
}

fn java_annotation_name_at(text: &str, at: usize) -> Option<(String, usize)> {
    if text.as_bytes().get(at) != Some(&b'@') {
        return None;
    }
    let mut end = at + 1;
    while text
        .as_bytes()
        .get(end)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
    {
        end += 1;
    }
    (end > at + 1).then(|| (text[at + 1..end].to_string(), end))
}

fn annotation_arg_string(annotation: &str, keys: &[&str]) -> Option<String> {
    let open = annotation.find('(')?;
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    for part in split_top_level_commas(args) {
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part.trim()
        } else {
            continue;
        };
        if let Some(literal) = first_raw_string_literal(value) {
            return Some(literal);
        }
    }
    None
}

fn first_raw_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            return read_raw_string_literal(text, idx).map(|(literal, _)| literal);
        }
        idx += 1;
    }
    None
}

fn read_raw_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            escape = false;
            i += ch.len_utf8();
            continue;
        }
        if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    None
}

fn is_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn strip_package(qn: &str) -> &str {
    qn.rsplit('.').next().unwrap_or(qn)
}
