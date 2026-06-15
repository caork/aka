use std::collections::BTreeMap;
use std::path::Path;

use super::persistence_access_synth::{table_access_edges_for_sql, TableAccessRef};
use super::{read_repo_text, EdgeRec, ProjectSourceSet, SynthNode};

pub(super) fn detect_mybatis_xml_table_access_edges(
    repo: &Path,
    project_sources: &ProjectSourceSet,
    nodes: &BTreeMap<String, SynthNode>,
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<EdgeRec> {
    let method_nodes = mybatis_method_nodes(nodes);
    let mut out = Vec::new();
    for file_path in project_sources
        .project_files(repo)
        .filter(|path| is_mybatis_xml_path(path))
    {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let Some(namespace) = xml_attr_value(&text, "mapper", "namespace") else {
            continue;
        };
        for statement in mybatis_xml_statements(&text) {
            let Some(node_id) = method_nodes
                .get(&format!("{namespace}.{}", statement.id))
                .or_else(|| {
                    method_nodes.get(&format!("{}.{}", strip_package(&namespace), statement.id))
                })
            else {
                continue;
            };
            out.extend(table_access_edges_for_sql(
                node_id,
                &statement.sql,
                table_lookup,
                &format!("java-mybatis-xml-{}", statement.tag),
            ));
        }
    }
    out
}

fn mybatis_method_nodes(nodes: &BTreeMap<String, SynthNode>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for node in nodes
        .values()
        .filter(|node| matches!(node.label.as_str(), "Method" | "Function"))
    {
        out.insert(node.qn.clone(), node.aka_id.clone());
        if let Some(parent) = &node.parent_class {
            out.insert(format!("{parent}.{}", node.name), node.aka_id.clone());
            out.insert(
                format!("{}.{}", strip_package(parent), node.name),
                node.aka_id.clone(),
            );
        }
    }
    out
}

struct MybatisXmlStatement {
    tag: String,
    id: String,
    sql: String,
}

fn mybatis_xml_statements(text: &str) -> Vec<MybatisXmlStatement> {
    let mut out = Vec::new();
    for tag in ["select", "insert", "update", "delete"] {
        let mut offset = 0usize;
        let open_prefix = format!("<{tag}");
        let close_tag = format!("</{tag}>");
        while let Some(rel) = text[offset..].find(&open_prefix) {
            let open = offset + rel;
            let Some(open_end_rel) = text[open..].find('>') else {
                break;
            };
            let open_end = open + open_end_rel;
            let attrs = &text[open..=open_end];
            let Some(id) = xml_attr_value(attrs, tag, "id") else {
                offset = open_end + 1;
                continue;
            };
            let Some(close_rel) = text[open_end + 1..].find(&close_tag) else {
                offset = open_end + 1;
                continue;
            };
            let close = open_end + 1 + close_rel;
            out.push(MybatisXmlStatement {
                tag: tag.to_string(),
                id,
                sql: text[open_end + 1..close].to_string(),
            });
            offset = close + close_tag.len();
        }
    }
    out
}

fn xml_attr_value(text: &str, tag: &str, attr: &str) -> Option<String> {
    let tag_start = text.find(&format!("<{tag}"))?;
    let tag_end = text[tag_start..].find('>').map(|rel| tag_start + rel)?;
    let tag_text = &text[tag_start..tag_end];
    let needle = format!("{attr}=");
    let attr_pos = tag_text.find(&needle)? + needle.len();
    let quote = tag_text.as_bytes().get(attr_pos).copied()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let value_start = attr_pos + 1;
    let value_end_rel = tag_text[value_start..].find(quote as char)?;
    Some(tag_text[value_start..value_start + value_end_rel].to_string())
}

fn is_mybatis_xml_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".xml") && (lower.contains("/mapper/") || lower.contains("/mappers/"))
}

fn strip_package(qn: &str) -> &str {
    qn.rsplit('.').next().unwrap_or(qn)
}
