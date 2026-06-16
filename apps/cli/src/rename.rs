use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use aka_mcp::{RenameEdit, SearchHit, SymbolRef};
use anyhow::{bail, Context, Result};

pub fn validate_identifier_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("rename replacement cannot be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        bail!("rename replacement must start with a letter or underscore: {name:?}");
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        bail!("rename replacement must be an identifier-like name: {name:?}");
    }
    Ok(())
}

pub fn collect_file_ranges(
    defs: &[SearchHit],
    refs: &[SymbolRef],
) -> BTreeMap<String, Vec<RangeInclusive<u32>>> {
    let mut ranges: BTreeMap<String, Vec<RangeInclusive<u32>>> = BTreeMap::new();
    for def in defs {
        if !def.file_path.is_empty() && def.start_line > 0 {
            ranges
                .entry(def.file_path.clone())
                .or_default()
                .push(line_window(def.start_line));
        }
    }
    for r in refs {
        if !r.file_path.is_empty() && r.start_line > 0 {
            ranges
                .entry(r.file_path.clone())
                .or_default()
                .push(line_window(r.start_line));
        }
    }
    ranges
}

pub fn apply_file_plan(
    repo_root: &Path,
    file_path: &str,
    ranges: &[RangeInclusive<u32>],
    target: &str,
    replacement: &str,
    dry_run: bool,
) -> Result<Option<Vec<RenameEdit>>> {
    let root = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize repo root {}", repo_root.display()))?;
    let abs = root.join(file_path).canonicalize().ok();
    let Some(abs) = abs else {
        return Ok(None);
    };
    if !abs.starts_with(&root) || !abs.is_file() {
        return Ok(None);
    }

    let original = std::fs::read_to_string(&abs)
        .with_context(|| format!("read source for rename {}", abs.display()))?;
    let original_lines: Vec<&str> = original.split('\n').collect();
    let mut lines: Vec<String> = original_lines
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    let line_numbers = expanded_line_numbers(ranges, lines.len());
    let mut edits = Vec::new();

    for line_no in line_numbers {
        let idx = (line_no - 1) as usize;
        let before = original_lines[idx];
        let positions = identifier_ranges(before, target);
        if positions.is_empty() {
            continue;
        }

        let line = &mut lines[idx];
        for pos in positions.iter().rev() {
            line.replace_range(*pos..*pos + target.len(), replacement);
        }
        edits.push(RenameEdit {
            file_path: file_path.to_string(),
            line: line_no,
            column: (positions[0] + 1) as u32,
            before: before.to_string(),
            after: line.clone(),
        });
    }

    if !dry_run && !edits.is_empty() {
        write_preserving_trailing_newline(&abs, &original, &lines)?;
    }

    Ok((!edits.is_empty()).then_some(edits))
}

fn line_window(line: u32) -> RangeInclusive<u32> {
    line.saturating_sub(2).max(1)..=line.saturating_add(2)
}

fn expanded_line_numbers(ranges: &[RangeInclusive<u32>], line_count: usize) -> Vec<u32> {
    let mut out = BTreeSet::new();
    let max = line_count as u32;
    for range in ranges {
        for line in range.clone() {
            if (1..=max).contains(&line) {
                out.insert(line);
            }
        }
    }
    out.into_iter().collect()
}

fn write_preserving_trailing_newline(
    abs: &PathBuf,
    original: &str,
    lines: &[String],
) -> Result<()> {
    let mut updated = lines.join("\n");
    if original.ends_with('\n') && !updated.ends_with('\n') {
        updated.push('\n');
    }
    std::fs::write(abs, updated).with_context(|| format!("write renamed source {}", abs.display()))
}

fn identifier_ranges(line: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = line[offset..].find(needle) {
        let idx = offset + pos;
        let before_ok = idx == 0
            || !line[..idx]
                .chars()
                .next_back()
                .is_some_and(is_identifier_char);
        let after = idx + needle.len();
        let after_ok =
            after >= line.len() || !line[after..].chars().next().is_some_and(is_identifier_char);
        if before_ok && after_ok {
            out.push(idx);
        }
        offset = after;
    }
    out
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_ranges_respect_boundaries() {
        assert_eq!(
            identifier_ranges("foo foobar bar_foo foo2 foo", "foo"),
            vec![0, 24]
        );
    }

    #[test]
    fn replacement_does_not_chain_when_new_name_contains_old_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src.rs");
        std::fs::write(&file, "fn foo() { foo(); }\n").unwrap();

        let edits = apply_file_plan(dir.path(), "src.rs", &[1..=1], "foo", "foobar", false)
            .unwrap()
            .unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "fn foobar() { foobar(); }\n"
        );
    }

    #[test]
    fn overlapping_windows_are_deduped_per_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src.rs");
        std::fs::write(&file, "foo();\n").unwrap();

        let edits = apply_file_plan(dir.path(), "src.rs", &[1..=1, 1..=2], "foo", "bar", true)
            .unwrap()
            .unwrap();

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].before, "foo();");
        assert_eq!(edits[0].after, "bar();");
    }
}
