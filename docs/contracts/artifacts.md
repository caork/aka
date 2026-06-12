# Engine 工件合同 v0

codebase-memory-mcp engine adapter 与 aka（Rust core）之间唯一的接口。aka 只消费本合同描述的文件与事件，**永不 import engine 内部模块**；engine 来源或 CBM SQLite schema 漂移时，只要 adapter 导出的合同不变，aka 搜索/图/服务层零改动。

## 调用方式

EngineRunner 调用 CBM 原生二进制：

```bash
codebase-memory-mcp cli --progress --json index_repository '{"repo_path":"<repoPath>","mode":"fast","persistence":false}'
```

随后 `aka-core` 从 `CBM_CACHE_DIR` 下的 CBM SQLite 读取图数据，导出 `<artifactDir>`：

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
  "engineVersion": "<codebase-memory-mcp sha 或 binary version>",
  "repoPath": "/abs/path",
  "commit": "<git HEAD sha 或 null>",
  "generatedAt": "ISO-8601",
  "stats": { "files": 0, "nodes": 0, "edges": 0, "chunks": 0 }
}
```

### nodes.ndjson — aka contract `GraphNode`

```json
{"id":"...","label":"Function","properties":{"name":"...","filePath":"...","startLine":1,"endLine":20}}
```

`label` 取值来自 CBM graph，经 adapter 规范化为 aka 节点标签（Project/Package/Folder/File/Module/Class/Function/Method/Interface/Enum/Type/Route/Resource/…）。`properties` 为开放对象，aka 侧只索引已知字段（name/filePath/startLine/endLine/…），其余落到 JSON 列存底。

**行号语义**：工件中的 `startLine`/`endLine`（节点与 chunk 同）由 CBM/tree-sitter 坐标导出，合同内保持 **0-based row**。Rust 摄取层（aka-graph/aka-search）写索引时统一 **+1 转为 1-based 人类行号**（`NodeRec::start_line_1based`），因此 SQLite/tantivy 及一切下游（HTTP/MCP/桌面端）的行号都与编辑器、`/api/source` 对齐。`properties` JSON 列存底里保留的是工件原始 0-based 值。

**合成 Process**：若 CBM SQLite 已经产出 `label = "Process"` 节点，adapter 原样透传，不重复合成；若没有，adapter 会基于 `CALLS` 边保守合成 `label = "Process"` 的调用链流程节点。合成节点的 `id` 形如 `process:call-chain:<hash>`，`properties` 至少包含 `name`、`processType = "call-chain"`、`stepCount`、`entryPointId`、`terminalId`、`source = "aka-cbm-synth"`。这是合同内只增字段/节点类型，下游按普通节点摄取。

### edges.ndjson — aka contract `GraphRelationship`

```json
{"id":"...","sourceId":"...","targetId":"...","type":"CALLS","confidence":0.9,"reason":"...","step":1,"evidence":[{"kind":"...","weight":0.5}]}
```

`type` 取值来自 CBM graph，经 adapter 规范化为 aka 关系类型（CONTAINS/DEFINES/CALLS/IMPORTS/INHERITS/IMPLEMENTS/HTTP_CALLS/READS/WRITES/…）。`step`/`evidence` 可选，缺失时下游按普通图边处理。

合成 Process 会追加两类边：

- `ENTRY_POINT_OF`：入口符号 → Process，`step` 为空。
- `STEP_IN_PROCESS`：流程步骤符号 → Process，`step` 为 1-based 步号。

这两类边的 `evidence.source` 为 `aka-cbm-synth`。aka-graph/aka-mcp 使用它们展示流程归属、流程步骤和 impact 的 affected_processes。

### chunks.ndjson — embedding 切块（向量由 aka 侧按需计算，engine 不算向量）

```json
{"nodeId":"...","kind":"ast-function","filePath":"...","startLine":1,"endLine":20,"text":"..."}
```

`kind`: `ast-function` | `ast-declaration` | `char`。CBM 不负责向量计算，embedding 仍由 aka 侧按 per-repo 设置异步回填。

## 进度事件（engine stdout，NDJSON，每行一个事件）

```json
{"event":"phase","phase":"codebase-memory:index","current":0,"total":0}
{"event":"phase","phase":"codebase-memory:export-artifacts","current":0,"total":0}
{"event":"warning","message":"..."}
{"event":"done","stats":{"files":0,"nodes":0,"edges":0,"chunks":0}}
```

`phase` 取值由 aka-core adapter 定义，当前至少包含 `codebase-memory:index` 与 `codebase-memory:export-artifacts`。stderr 仅用于人类可读日志，aka 不解析。

## 版本纪律

- 字段只增不改不删；破坏性变更必须 `contractVersion` +1，aka 侧同步适配后才能合入。
- CBM 新增 label/type 时属于"只增"，adapter 可透传或规范化；aka 侧未知 label/type 落通用表，不报错。
