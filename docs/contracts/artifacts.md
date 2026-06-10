# Engine 工件合同 v0

engine（TS sidecar）与 aka（Rust core）之间唯一的接口。aka 只消费本合同描述的文件与事件，**永不 import engine 内部模块**；engine 跟随上游 rebase 时只要合同不变，aka 零改动。

## 调用方式

```
bun run engine/emit.ts --repo <repoPath> --out <artifactDir> [--no-chunks]
```

退出码 0 = 成功且工件完整；非 0 = 失败，工件目录视为不可信。

## 工件目录

```
<artifactDir>/
  manifest.json    # 元信息与统计，最后写入（作为完成标记）
  nodes.ndjson     # 每行一个节点
  edges.ndjson     # 每行一条边
  chunks.ndjson    # 每行一个 embedding 切块（--no-chunks 时省略）
```

manifest.json 最后写入：aka 侧以 manifest 存在且 `contractVersion` 匹配作为"工件完整"的判据。

### manifest.json

```json
{
  "contractVersion": 0,
  "engineVersion": "1.6.7+aka.1",
  "repoPath": "/abs/path",
  "commit": "<git HEAD sha 或 null>",
  "generatedAt": "ISO-8601",
  "stats": { "files": 0, "nodes": 0, "edges": 0, "chunks": 0 }
}
```

### nodes.ndjson — gitnexus-shared `GraphNode` 原样透传

```json
{"id":"...","label":"Function","properties":{"name":"...","filePath":"...","startLine":1,"endLine":20}}
```

`label` 取值即上游 `NodeLabel`（File/Folder/Function/Class/Interface/Method/CodeElement/Community/Process/Struct/Enum/Trait/…）。`properties` 为开放对象，aka 侧只索引已知字段（name/filePath/startLine/endLine/…），其余落到 JSON 列存底。

**行号语义**：工件中的 `startLine`/`endLine`（节点与 chunk 同）是 engine（tree-sitter）的 **0-based row**——这是工件原样透传的语义，不改。Rust 摄取层（aka-graph/aka-search）写索引时统一 **+1 转为 1-based 人类行号**（`NodeRec::start_line_1based`），因此 SQLite/tantivy 及一切下游（HTTP/MCP/桌面端）的行号都与编辑器、`/api/source` 对齐。`properties` JSON 列存底里保留的是工件原始 0-based 值。

### edges.ndjson — gitnexus-shared `GraphRelationship` 原样透传

```json
{"id":"...","sourceId":"...","targetId":"...","type":"CALLS","confidence":0.9,"reason":"...","step":1,"evidence":[{"kind":"...","weight":0.5}]}
```

`type` 取值即上游 `RelationshipType`（CONTAINS/DEFINES/CALLS/IMPORTS/EXTENDS/…/WRAPS/QUERIES）。`step`/`evidence` 可选。

### chunks.ndjson — embedding 切块（向量由 aka 侧按需计算，engine 不算向量）

```json
{"nodeId":"...","kind":"ast-function","filePath":"...","startLine":1,"endLine":20,"text":"..."}
```

`kind`: `ast-function` | `ast-declaration` | `char`（对应上游三种切块策略）。

## 进度事件（engine stdout，NDJSON，每行一个事件）

```json
{"event":"phase","phase":"parse","current":120,"total":900}
{"event":"warning","message":"..."}
{"event":"done","stats":{"files":0,"nodes":0,"edges":0,"chunks":0}}
```

`phase` 取值 = 管线 13 阶段名（scan/structure/markdown/cobol/parse/routes/tools/orm/crossFile/scopeResolution/mro/communities/processes）。stderr 仅用于人类可读日志，aka 不解析。

## 版本纪律

- 字段只增不改不删；破坏性变更必须 `contractVersion` +1，aka 侧同步适配后才能合入。
- engine rebase 上游后若上游新增 NodeLabel/RelationshipType，属于"只增"，aka 侧未知 label 落通用表，不报错。
