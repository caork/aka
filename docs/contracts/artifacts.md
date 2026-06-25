# Engine facts contract v1

AKA 的运行时合同是 `aka-facts`：parser / engine / SCIP importer / stack-graphs adapter 产出稳定 facts，Rust 侧直接把这些 facts 写入 graph/search。旧的 `artifact/` 目录、facts sidecar NDJSON 和 engine SQLite adapter 不再作为兼容或调试 transport。

目标数据流：

```
repo files -> parallel fact producers -> aka-facts::FactSource
  -> graph/search writer -> SQLite graph.db + tantivy search + CSR projection
```

## Fact source

`FactSource` 是可重放的事实来源。当前 graph/search writer 会读取 nodes 两次，所以 direct engine producer 可以先落成内存 batch、replayable spool 或调试文件；最终 one-pass writer 完成后再收紧这个约束。

```rust
trait FactSource {
    fn stats(&self) -> &FactStats;
    fn nodes(&self) -> Iterator<Item = Result<NodeFact>>;
    fn edges(&self) -> Iterator<Item = Result<EdgeFact>>;
    fn chunks(&self) -> Option<Iterator<Item = Result<ChunkFact>>>;
}
```

事实合同版本为 `aka_facts::FACTS_VERSION = 1`。`contractVersion = 0` 的旧 artifact 目录不再是运行时输入。

## Fact records

### `FactManifest`

```json
{
  "contractVersion": 1,
  "engineVersion": "<engine/library/indexer version>",
  "repoPath": "/abs/path",
  "commit": "<git HEAD sha or null>",
  "generatedAt": "ISO-8601",
  "stats": { "files": 0, "nodes": 0, "edges": 0, "chunks": 0 }
}
```

Direct facts 不依赖 `manifest.json` 作为完成标记。一个 producer 完成时必须发送 `Done { stats }` 或返回完整 `FactBatch`；调试导出才需要把 manifest 写到磁盘。默认索引入口要求 producer 产出**完整仓库 facts**。当前增量 writer 会从完整 facts 中按 file delta 安全切片；producer 只吐 changed files 会丢跨文件边和 parse-cache ownership，必须显式进入未来的增量合并协议，不能伪装成完整 facts。

### `NodeFact`

```json
{"id":"...","label":"Function","properties":{"name":"...","filePath":"...","startLine":0,"endLine":19}}
```

`label` 是 AKA 图层节点标签，至少包括 Project/Package/Folder/File/Module/Class/Function/Method/Interface/Enum/Type/Route/GraphQL/Tool/Command/Config/Topic/Channel/Table/Repository/Migration/Resource/Transaction/Process/Community。未知 label 必须透传，下游落通用表，不报错。

`properties` 是开放 JSON map。稳定字段：

- `name`
- `qualifiedName`
- `symbol`，SCIP-like symbol id，可选
- `filePath`
- `path`
- `language`
- `startLine`
- `endLine`
- `startCol`
- `endCol`

行号语义保持不变：facts 中的 `startLine` / `endLine` 是 parser/tree-sitter 坐标的 **0-based row**。aka-graph / aka-search 写索引时统一转换为 1-based 人类行号。

### `EdgeFact`

```json
{"id":"...","sourceId":"...","targetId":"...","type":"CALLS","confidence":0.9,"reason":"...","step":1,"evidence":{"source":"scip"}}
```

`type` 是 AKA 图层关系类型，至少包括 CONTAINS/DEFINES/CALLS/IMPORTS/INHERITS/IMPLEMENTS/DEPENDS_ON/HTTP_CALLS/ACCESSES_RESOURCE/FETCHES/HANDLES_ROUTE/HANDLES_GRAPHQL/HANDLES_TOOL/HANDLES_COMMAND/HANDLES_JOB/ENQUEUES_JOB/USES_STEP/USES_CONFIG/CONSUMES_TOPIC/PUBLISHES_TOPIC/HAS_TRANSACTION_BOUNDARY/MAPS_TO_TABLE/REPOSITORY_FOR/MIGRATES_TABLE/READS_TABLE/WRITES_TABLE/READS/WRITES/MEMBER_OF/ENTRY_POINT_OF/STEP_IN_PROCESS。未知 type 必须透传。

`step` 只用于有序流程边，例如 `STEP_IN_PROCESS`。`evidence` 保留 source、rule、confidence breakdown、SCIP occurrence role、stack-graphs path 等调试依据。

### `ChunkFact`

```json
{"kind":"chunk","nodeId":"...","chunkKind":"ast-function","filePath":"...","startLine":0,"endLine":19,"text":"..."}
```

在 `FactRecord` JSONL envelope 中，顶层 `kind` 表示 record 类型；chunk 自身的类型字段写作 `chunkKind`，进入内存 `ChunkFact.kind` 后仍是 `ast-function` | `ast-declaration` | `char` | producer 自定义字符串。向量 embedding 仍由 AKA 设置控制，默认关闭；chunk 只是索引候选文本。

## Semantic facts

`aka-facts` 还定义 SCIP/Glean-like 上层语义记录：

- `FileFact`
- `SymbolFact`
- `OccurrenceFact`
- `RelationFact`
- `TextRange`

这些记录先表达 parser/indexer 的真实语义，再 lower 成 `NodeFact` / `EdgeFact` / `ChunkFact` 进入现有 graph/search。Occurrence 默认不膨胀为图节点；definition/declaration occurrence 可以补 symbol range，reference/call occurrence 可以生成 relation evidence。

## Producer ownership

优先级从高到低：

1. Native AKA engine embedded/direct API：负责基础 parse、symbols、defs/refs、imports、语言原生结构。
2. SCIP importer：导入已有语言 indexer 的 symbols/occurrences/relations。
3. tree-sitter stack-graphs adapter：负责轻量 name resolution / scope graph。
4. LSP adapter：只作为 unsupported language 或 live assist，不作为批量索引主路径。
5. AKA enrichment jobs：Route/GraphQL/Tool/Command/Config/Topic/Table/Migration/Transaction/Process/Community 等业务语义。它们必须异步或有预算，不能阻塞 baseline index ready。

## Progress events

新热路径使用 facts 阶段名：

```json
{"event":"phase","phase":"aka-engine:facts:discover","current":0,"total":0}
{"event":"phase","phase":"aka-engine:facts:parse","current":10,"total":100}
{"event":"phase","phase":"aka-core:fuse-facts:graph","current":0,"total":0}
{"event":"phase","phase":"index:graph:nodes","current":0,"total":0}
{"event":"done","stats":{"files":0,"nodes":0,"edges":0,"chunks":0}}
```

不再定义 `aka-engine:export-artifacts:*` 运行阶段。UI 应把 facts / enrichment / index 分开展示，不再把多个语义阶段折叠成一个 “artifacts” 阶段。

## Removed legacy artifact transport

旧 `artifact/` 目录曾是兼容格式：

```
artifact/
  manifest.json
  nodes.ndjson
  edges.ndjson
  chunks.ndjson
```

`manifest.json` 的 `contractVersion` 为 `0`，`nodes.ndjson` / `edges.ndjson` / `chunks.ndjson` 的 JSON shape 与 `NodeFact` / `EdgeFact` / `ChunkFact` 接近，但这一路径已退出产品/runtime。新功能不得依赖 engine SQLite schema、legacy NDJSON 文件或 `ArtifactDir` adapter；需要排障时应从 embedded/direct facts producer 增加有边界的诊断输出，而不是恢复旧 adapter。

## Version discipline

- facts 字段只增不改不删；破坏性变更必须 bump `FACTS_VERSION`。
- legacy artifact v0 不承载运行时能力。
- engine 内部 SQLite schema 不是 aka-core 合同。schema 漂移不能影响 direct facts writer。
