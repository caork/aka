use std::path::Path;

use super::{line_number_at_offset, stable_hash, SynthNode};

pub(super) fn python_source_symbols(file_path: &str, text: &str) -> Vec<SynthNode> {
    let module = python_module_name(file_path);
    let declarations = python_declarations(text);
    let mut out = Vec::new();
    for decl in declarations
        .iter()
        .filter(|decl| decl.kind == PyDeclKind::Class && decl.indent == 0)
    {
        let qn = python_qn(&module, None, &decl.name);
        out.push(source_node(SourceNodeSpec {
            label: "Class",
            name: decl.name.clone(),
            qn: qn.clone(),
            file_path,
            start: decl.start,
            end: decl.end,
            decorators: decl.decorators.clone(),
            parent_class: None,
            text,
        }));
        for method in declarations.iter().filter(|candidate| {
            matches!(candidate.kind, PyDeclKind::Function)
                && candidate.indent > decl.indent
                && candidate.start > decl.start
                && candidate.start < decl.end
                && direct_child_indent(text, decl, candidate)
        }) {
            out.push(source_node(SourceNodeSpec {
                label: "Method",
                name: method.name.clone(),
                qn: python_qn(&module, Some(&qn), &method.name),
                file_path,
                start: method.start,
                end: method.end,
                decorators: method.decorators.clone(),
                parent_class: Some(qn.clone()),
                text,
            }));
        }
    }

    for function in declarations.iter().filter(|function| {
        matches!(function.kind, PyDeclKind::Function)
            && function.indent == 0
            && !declarations.iter().any(|class| {
                class.kind == PyDeclKind::Class
                    && function.indent > class.indent
                    && function.start > class.start
                    && function.start < class.end
            })
    }) {
        out.push(source_node(SourceNodeSpec {
            label: "Function",
            name: function.name.clone(),
            qn: python_qn(&module, None, &function.name),
            file_path,
            start: function.start,
            end: function.end,
            decorators: function.decorators.clone(),
            parent_class: None,
            text,
        }));
    }

    out.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.qn.cmp(&b.qn))
    });
    out
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PyDeclKind {
    Class,
    Function,
}

#[derive(Debug, Clone)]
struct PyDecl {
    kind: PyDeclKind,
    name: String,
    indent: usize,
    start: usize,
    end: usize,
    decorators: Vec<String>,
}

fn python_declarations(text: &str) -> Vec<PyDecl> {
    let line_offsets = line_offsets(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut declarations = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len().saturating_sub(trimmed.len());
        let Some((kind, name)) = python_decl_name(trimmed) else {
            continue;
        };
        let start_line = decorator_start_line(&lines, idx);
        let start = line_offsets.get(start_line).copied().unwrap_or(0);
        let end_line = python_block_end_line(&lines, idx, indent);
        let end = line_offsets.get(end_line).copied().unwrap_or(text.len());
        declarations.push(PyDecl {
            kind,
            name,
            indent,
            start,
            end,
            decorators: decorators_before(&lines, idx),
        });
    }
    declarations
}

fn python_decl_name(line: &str) -> Option<(PyDeclKind, String)> {
    if let Some(rest) = line.strip_prefix("class ") {
        let name = read_python_identifier(rest)?;
        return Some((PyDeclKind::Class, name));
    }
    if let Some(rest) = line
        .strip_prefix("def ")
        .or_else(|| line.strip_prefix("async def "))
    {
        let name = read_python_identifier(rest)?;
        return Some((PyDeclKind::Function, name));
    }
    None
}

fn read_python_identifier(text: &str) -> Option<String> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut end = first.len_utf8();
    for (idx, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            break;
        }
        end = idx + ch.len_utf8();
    }
    Some(text[..end].to_string())
}

fn decorator_start_line(lines: &[&str], mut line_idx: usize) -> usize {
    while line_idx > 0 {
        let prev = lines[line_idx - 1].trim_start();
        if !prev.starts_with('@') {
            break;
        }
        line_idx -= 1;
    }
    line_idx
}

fn decorators_before(lines: &[&str], line_idx: usize) -> Vec<String> {
    let start = decorator_start_line(lines, line_idx);
    lines[start..line_idx]
        .iter()
        .map(|line| line.trim().to_string())
        .filter(|line| line.starts_with('@'))
        .collect()
}

fn python_block_end_line(lines: &[&str], start_idx: usize, indent: usize) -> usize {
    let mut end = start_idx + 1;
    while end < lines.len() {
        let line = lines[end];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            end += 1;
            continue;
        }
        let current_indent = line.len().saturating_sub(line.trim_start().len());
        if current_indent <= indent {
            break;
        }
        end += 1;
    }
    end
}

fn direct_child_indent(text: &str, class: &PyDecl, method: &PyDecl) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let method_line = line_number_at_offset(text, method.start).saturating_sub(1) as usize;
    lines
        .get(method_line)
        .map(|line| line.len().saturating_sub(line.trim_start().len()) > class.indent)
        .unwrap_or(false)
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            offsets.push(idx + 1);
        }
    }
    offsets.push(text.len());
    offsets
}

struct SourceNodeSpec<'a> {
    label: &'a str,
    name: String,
    qn: String,
    file_path: &'a str,
    start: usize,
    end: usize,
    decorators: Vec<String>,
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
    SynthNode {
        aka_id: format!("source:python:{:016x}", stable_hash(&key)),
        qn: spec.qn,
        label: spec.label.into(),
        name: spec.name,
        file_path: spec.file_path.into(),
        start_line,
        end_line,
        language: "python".into(),
        route_path: None,
        route_method: None,
        decorators: spec.decorators,
        parent_class: spec.parent_class,
        is_exported: false,
        ast_framework_multiplier: 1.0,
        ast_framework_reason: None,
    }
}

fn python_qn(module: &str, owner: Option<&str>, name: &str) -> String {
    if let Some(owner) = owner.filter(|value| !value.is_empty()) {
        format!("{owner}.{name}")
    } else if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}.{name}")
    }
}

fn python_module_name(file_path: &str) -> String {
    let path = Path::new(file_path);
    let without_ext = path.with_extension("");
    let parts: Vec<_> = without_ext
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter(|part| *part != "__init__")
        .collect();
    parts.join(".")
}
