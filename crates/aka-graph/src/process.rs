//! Process 查询 — engine 流程检测产物（"入口→终点"调用链的合成节点）。
//!
//! 边方向约定：`符号 -[STEP_IN_PROCESS]-> Process`（步号在 edges.step，1-based）、
//! `入口 -[ENTRY_POINT_OF]-> Process`。流程查询是低频点查，SQL 直查
//! （edges JOIN nodes JOIN edge_types）即可，不进 CSR 邻接。

use std::collections::HashMap;

use rusqlite::{params, params_from_iter};

use crate::error::Result;
use crate::store::GraphStore;

/// 符号视角：它参与的一条流程及其在流程中的位置。
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessMembership {
    /// Process 节点的 nodes.rowid。
    pub process_rowid: i64,
    /// nodes.id，如 "proc_0_fullgeneration"。
    pub process_id: String,
    /// Process 节点 name，缺省 ""。
    pub name: String,
    /// 节点 props 的 processType，缺省 ""。
    pub process_type: String,
    /// 该符号在流程中的步号（edges.step，1-based）。
    pub step: Option<u32>,
    /// 节点 props 的 stepCount。
    pub step_count: Option<u32>,
}

/// 流程视角：一个步骤（源符号）及其步号。
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStepRow {
    pub rowid: i64,
    pub id: String,
    /// 缺省 ""。
    pub name: String,
    pub label: String,
    pub file_path: Option<String>,
    /// nodes 表统一存 1-based 行号。
    pub start_line: Option<u32>,
    pub step: Option<u32>,
}

/// 符号所属的社区/模块信息。
#[derive(Debug, Clone, PartialEq)]
pub struct CommunityMembership {
    /// Community 节点名或 heuristicLabel。
    pub module: String,
    /// Community props.cohesion，缺省 0。
    pub cohesion: f32,
}

impl GraphStore {
    /// 一个符号参与的所有流程：沿出边 STEP_IN_PROCESS 找 Process 节点，
    /// process_rowid 升序（确定性）。
    pub fn processes_of_node(&self, node_rowid: i64) -> Result<Vec<ProcessMembership>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT p.rowid, p.id, p.name, p.props, e.step \
             FROM edges e \
             JOIN edge_types t ON t.type_id = e.type_id \
             JOIN nodes p ON p.rowid = e.target \
             WHERE e.source = ?1 AND t.name = 'STEP_IN_PROCESS' AND p.label = 'Process' \
             ORDER BY p.rowid ASC",
        )?;
        let rows = stmt.query_map(params![node_rowid], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<u32>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (process_rowid, process_id, name, props_text, step) = row?;
            // props 解析失败按空对象处理（缺省字段语义不变）。
            let props: serde_json::Value =
                serde_json::from_str(&props_text).unwrap_or(serde_json::Value::Null);
            out.push(ProcessMembership {
                process_rowid,
                process_id,
                name: name.unwrap_or_default(),
                process_type: props["processType"].as_str().unwrap_or("").to_owned(),
                step,
                step_count: props["stepCount"].as_u64().map(|v| v as u32),
            });
        }
        Ok(out)
    }

    /// 一个入口节点触发的流程：沿出边 ENTRY_POINT_OF 找 Process 节点。
    ///
    /// Route/Tool 这类应用语义节点通常不会是 STEP_IN_PROCESS 的步骤，
    /// 但 GitNexus 会用 `Route|Tool -[ENTRY_POINT_OF]-> Process` 表达它们
    /// 触发哪些执行流。
    pub fn entry_processes_of_node(&self, node_rowid: i64) -> Result<Vec<ProcessMembership>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT p.rowid, p.id, p.name, p.props, e.step \
             FROM edges e \
             JOIN edge_types t ON t.type_id = e.type_id \
             JOIN nodes p ON p.rowid = e.target \
             WHERE e.source = ?1 AND t.name = 'ENTRY_POINT_OF' AND p.label = 'Process' \
             ORDER BY p.rowid ASC",
        )?;
        let rows = stmt.query_map(params![node_rowid], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<u32>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (process_rowid, process_id, name, props_text, step) = row?;
            let props: serde_json::Value =
                serde_json::from_str(&props_text).unwrap_or(serde_json::Value::Null);
            out.push(ProcessMembership {
                process_rowid,
                process_id,
                name: name.unwrap_or_default(),
                process_type: props["processType"].as_str().unwrap_or("").to_owned(),
                step,
                step_count: props["stepCount"].as_u64().map(|v| v as u32),
            });
        }
        Ok(out)
    }

    /// 批量查询符号参与的流程，避免 query 对每个命中单独查一次 SQLite。
    /// 输出 key 是传入的 node id。
    pub fn processes_of_node_ids(
        &self,
        node_ids: &[String],
    ) -> Result<HashMap<String, Vec<ProcessMembership>>> {
        let mut out: HashMap<String, Vec<ProcessMembership>> = HashMap::new();
        for ids in node_ids.chunks(100) {
            if ids.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT n.id, p.rowid, p.id, p.name, p.props, e.step \
                 FROM edges e \
                 JOIN edge_types t ON t.type_id = e.type_id \
                 JOIN nodes n ON n.rowid = e.source \
                 JOIN nodes p ON p.rowid = e.target \
                 WHERE n.id IN ({placeholders}) AND t.name = 'STEP_IN_PROCESS' AND p.label = 'Process' \
                 ORDER BY n.id ASC, p.rowid ASC"
            );
            let mut stmt = self.conn().prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(ids.iter().map(String::as_str)), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<u32>>(5)?,
                ))
            })?;
            for row in rows {
                let (node_id, process_rowid, process_id, name, props_text, step) = row?;
                let props: serde_json::Value =
                    serde_json::from_str(&props_text).unwrap_or(serde_json::Value::Null);
                out.entry(node_id).or_default().push(ProcessMembership {
                    process_rowid,
                    process_id,
                    name: name.unwrap_or_default(),
                    process_type: props["processType"].as_str().unwrap_or("").to_owned(),
                    step,
                    step_count: props["stepCount"].as_u64().map(|v| v as u32),
                });
            }
        }
        Ok(out)
    }

    /// 批量查询符号所属的第一个 Community，保持 GitNexus query 的 module/cohesion 语义。
    pub fn community_of_node_ids(
        &self,
        node_ids: &[String],
    ) -> Result<HashMap<String, CommunityMembership>> {
        let mut out = HashMap::new();
        for ids in node_ids.chunks(100) {
            if ids.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT n.id, c.name, c.props \
                 FROM edges e \
                 JOIN edge_types t ON t.type_id = e.type_id \
                 JOIN nodes n ON n.rowid = e.source \
                 JOIN nodes c ON c.rowid = e.target \
                 WHERE n.id IN ({placeholders}) AND t.name = 'MEMBER_OF' AND c.label = 'Community' \
                 ORDER BY n.id ASC, c.rowid ASC"
            );
            let mut stmt = self.conn().prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(ids.iter().map(String::as_str)), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (node_id, name, props_text) = row?;
                if out.contains_key(&node_id) {
                    continue;
                }
                let props: serde_json::Value =
                    serde_json::from_str(&props_text).unwrap_or(serde_json::Value::Null);
                let module = props["heuristicLabel"]
                    .as_str()
                    .or_else(|| props["label"].as_str())
                    .or(name.as_deref())
                    .unwrap_or("")
                    .to_string();
                out.insert(
                    node_id,
                    CommunityMembership {
                        module,
                        cohesion: props["cohesion"].as_f64().unwrap_or(0.0) as f32,
                    },
                );
            }
        }
        Ok(out)
    }

    /// 一条流程的全部步骤：沿入边 STEP_IN_PROCESS 取源符号，
    /// step 升序（NULL 排最后），再按 rowid 升序保证确定性。
    pub fn process_steps(&self, process_rowid: i64) -> Result<Vec<ProcessStepRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT s.rowid, s.id, s.name, s.label, s.file_path, s.start_line, e.step \
             FROM edges e \
             JOIN edge_types t ON t.type_id = e.type_id \
             JOIN nodes s ON s.rowid = e.source \
             WHERE e.target = ?1 AND t.name = 'STEP_IN_PROCESS' \
             ORDER BY (e.step IS NULL) ASC, e.step ASC, s.rowid ASC",
        )?;
        let rows = stmt.query_map(params![process_rowid], |r| {
            Ok(ProcessStepRow {
                rowid: r.get(0)?,
                id: r.get(1)?,
                name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                label: r.get(3)?,
                file_path: r.get(4)?,
                start_line: r.get(5)?,
                step: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
