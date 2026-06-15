use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::{aka_node_id, stable_hash, text_col, EdgeRec, EngineError, NodeRec, SourceCache};

#[derive(Debug, Clone)]
pub(super) struct SynthProperty {
    pub(super) id: String,
    pub(super) owner_id: String,
    pub(super) owner_name: String,
    pub(super) name: String,
    pub(super) declared_type: Option<String>,
    pub(super) file_path: String,
    pub(super) start_line: u32,
    pub(super) end_line: u32,
}

impl SynthProperty {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("qualifiedName".into(), Value::String(self.id.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("startLine".into(), Value::from(self.start_line));
        properties.insert("endLine".into(), Value::from(self.end_line));
        properties.insert("ownerId".into(), Value::String(self.owner_id.clone()));
        properties.insert("ownerName".into(), Value::String(self.owner_name.clone()));
        properties.insert(
            "source".into(),
            Value::String("aka-python-property-synth".into()),
        );
        if let Some(declared_type) = &self.declared_type {
            properties.insert("declaredType".into(), Value::String(declared_type.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Property".into(),
            properties,
        }
    }

    pub(super) fn edge_rec(&self) -> EdgeRec {
        EdgeRec {
            id: format!("{}:has-property", self.id),
            source_id: self.owner_id.clone(),
            target_id: self.id.clone(),
            edge_type: "HAS_PROPERTY".into(),
            confidence: 0.82,
            reason: "aka Python class property synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-python-property-synth",
                "owner": self.owner_name,
                "declaredType": self.declared_type,
            })),
        }
    }
}

pub(super) fn synthesize_python_properties(
    conn: &Connection,
    project: &str,
    repo: &Path,
    existing_node_ids: &HashSet<String>,
) -> Result<Vec<SynthProperty>, EngineError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, qualified_name, file_path, start_line, end_line \
         FROM nodes \
         WHERE project = ?1 AND label = 'Class' AND file_path LIKE '%.py' AND file_path != '' \
         ORDER BY file_path, start_line, id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut sources = SourceCache::new(repo);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let owner_name = text_col(row, 1)?;
        let owner_qn = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        if owner_name.is_empty()
            || owner_name.starts_with('[')
            || start_line <= 0
            || end_line < start_line
        {
            continue;
        }
        let Some(text) = sources.read_file(&file_path) else {
            continue;
        };
        let owner_id = aka_node_id(cbm_id, &owner_qn);
        for prop in extract_python_class_properties(
            &text,
            &file_path,
            &owner_id,
            &owner_name,
            start_line as usize,
            end_line as usize,
        ) {
            if !existing_node_ids.contains(&prop.id) && seen.insert(prop.id.clone()) {
                out.push(prop);
            }
        }
    }
    Ok(out)
}

pub(super) fn extract_python_class_properties(
    text: &str,
    file_path: &str,
    owner_id: &str,
    owner_name: &str,
    start_line_1based: usize,
    end_line_1based: usize,
) -> Vec<SynthProperty> {
    let lines: Vec<&str> = text.lines().collect();
    if start_line_1based == 0 || start_line_1based > lines.len() {
        return Vec::new();
    }
    let class_idx = start_line_1based - 1;
    let class_indent = leading_spaces(lines[class_idx]);
    let mut out = Vec::new();
    let upper = end_line_1based.min(lines.len());
    for (line_no, line) in lines.iter().enumerate().take(upper).skip(class_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('@') {
            continue;
        }
        let indent = leading_spaces(line);
        if indent <= class_indent {
            break;
        }
        if indent != class_indent + 4 {
            continue;
        }
        let Some((name, declared_type)) = parse_python_property_line(trimmed) else {
            continue;
        };
        let key = format!("{owner_id}:{name}:{}", line_no + 1);
        out.push(SynthProperty {
            id: format!("python-property:{:016x}", stable_hash(&key)),
            owner_id: owner_id.to_string(),
            owner_name: owner_name.to_string(),
            name,
            declared_type,
            file_path: file_path.to_string(),
            start_line: line_no as u32,
            end_line: line_no as u32,
        });
    }
    out
}

fn parse_python_property_line(line: &str) -> Option<(String, Option<String>)> {
    if line.starts_with("def ")
        || line.starts_with("class ")
        || line.starts_with("async ")
        || line.starts_with("return ")
        || line.starts_with("pass")
    {
        return None;
    }
    let code = line.split('#').next()?.trim();
    let (left, right) = split_assignment_or_annotation(code)?;
    let name = left.trim();
    if !is_python_ident(name) || name.starts_with("__") {
        return None;
    }
    let declared_type = right
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches(',').to_string());
    Some((name.to_string(), declared_type))
}

fn split_assignment_or_annotation(code: &str) -> Option<(&str, Option<&str>)> {
    if let Some((name, rest)) = code.split_once(':') {
        let declared = rest.split('=').next().map(str::trim);
        return Some((name, declared));
    }
    let (name, rhs) = code.split_once('=')?;
    let rhs = rhs.trim_start();
    if rhs.starts_with("Column(")
        || rhs.starts_with("relationship(")
        || rhs.starts_with("mapped_column(")
        || rhs.starts_with("Field(")
    {
        return Some((name, None));
    }
    None
}

fn is_python_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}
