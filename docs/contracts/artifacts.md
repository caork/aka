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
- `source`，事实来源类别，例如 `engine` / `scip` / `stack-graphs` / `lsp`
- `provenance`，外部 OSS analyzer 产物的来源元数据，至少包括 `source`、`analyzerId`、`analyzerKind`、`tool`、`toolVersion`、`adapterVersion`、`oss`

行号语义保持不变：facts 中的 `startLine` / `endLine` 是 parser/tree-sitter 坐标的 **0-based row**。aka-graph / aka-search 写索引时统一转换为 1-based 人类行号。

### `EdgeFact`

```json
{"id":"...","sourceId":"...","targetId":"...","type":"CALLS","confidence":0.9,"reason":"...","step":1,"evidence":{"source":"scip"}}
```

`type` 是 AKA 图层关系类型，至少包括 CONTAINS/DEFINES/CALLS/IMPORTS/INHERITS/IMPLEMENTS/DEPENDS_ON/HTTP_CALLS/ACCESSES_RESOURCE/FETCHES/HANDLES_ROUTE/HANDLES_GRAPHQL/HANDLES_TOOL/HANDLES_COMMAND/HANDLES_JOB/ENQUEUES_JOB/USES_STEP/USES_CONFIG/CONSUMES_TOPIC/PUBLISHES_TOPIC/HAS_TRANSACTION_BOUNDARY/MAPS_TO_TABLE/REPOSITORY_FOR/MIGRATES_TABLE/READS_TABLE/WRITES_TABLE/READS/WRITES/MEMBER_OF/ENTRY_POINT_OF/STEP_IN_PROCESS。未知 type 必须透传。

`step` 只用于有序流程边，例如 `STEP_IN_PROCESS`。`evidence` 保留 source、rule、confidence breakdown、SCIP occurrence role、stack-graphs path 等调试依据。

外部 enrichment 产出的边必须在 `evidence` 内携带 provenance：

```json
{
  "source": "lsp",
  "rule": "references",
  "provenance": {
    "source": "lsp",
    "analyzerId": "pyright",
    "analyzerKind": "lsp",
    "tool": "Pyright",
    "toolVersion": "1.x",
    "adapterVersion": "0.1.x",
    "oss": true
  }
}
```

如果 analyzer 原始 evidence 不是 object，adapter 必须包成 `{ "value": <raw>, "source": "...", "provenance": ... }`，不能丢弃原始依据。

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

1. Native AKA engine embedded/direct API：负责 baseline parser facts：Project/Folder/File 结构、Module/Class/Function/Method 等定义节点、DEFINES/DEFINES_METHOD/IMPORTS 等可直接由解析结果确定的基础边。embedded/direct facts 默认 `baseline_facts_only=true`，不运行 engine 旧的 route/config/k8s/tests/git-history/similarity/complexity/call-heuristic/usages/semantic 推断 pass。
2. SCIP importer：导入已有语言 indexer 的 symbols/occurrences/relations。
3. tree-sitter stack-graphs adapter：负责轻量 name resolution / scope graph。
4. LSP adapter：只接成熟热门开源语言服务（优先 rust-analyzer、pyright、jdtls、typescript-language-server/tsserver、gopls），作为 baseline index ready 之后的可选事实源。

外部 enrichment provider 的入场条件：

- provider id 必须在 `aka-core::allowed_oss_analyzers()` 白名单内，当前只接受 `scip`、`stack-graphs`、`rust-analyzer`、`pyright`、`jdtls`、`typescript-language-server`、`gopls` 及代码中定义的兼容 alias。
- provider 必须是成熟开源分析器或其结果格式的 adapter；Rust 侧只能做转换、校验、合并和索引，不能新增多语言业务语义启发式扫描。
- provider 产出的 `NodeFact` / `EdgeFact` 在 merge 前必须通过 `AnalyzerRunMetadata` stamp provenance。runtime merge 入口会强制校验 OSS analyzer allowlist 和非空 `toolVersion`；没有 `source`、`analyzerId`、`toolVersion` / `adapterVersion` 的新增语义事实不能进入 graph/search。
- provider 只能在 baseline graph/search ready 之后运行。disabled、缺 provider、超时、异常、部分失败都只能记录 skipped/timeout/failed diagnostics，不能把仓库状态改为 failed，也不能阻塞已有 graph/search 查询；单个 provider 失败后应继续尝试后续 provider，直到有一个 provider 成功 merge 或全部 skipped/failed。
- provider merge 必须在 graph/search 的 staging 副本上完成；只有完整校验、写图、重建布局、写搜索并提交都成功后，才安装回正式索引。provider 失败、merge 失败或 deadline 触发时必须丢弃 staging，原 baseline graph/search 保持原样。
- provider 是否从可选走向默认可用，必须先通过 50 万行级别以上的大仓 indexing smoke 和查询回归；建议基准库包括 Apache Dubbo、CPython、Kubernetes、TypeScript、Spring Framework。

`aka-facts/scip-import` 只读取已经存在的 `index.scip` 并转换为 `FileFact` / `SymbolFact` / `OccurrenceFact` / `RelationFact`；它不启动语言 indexer，也不执行自研源码扫描。内部 runtime 的 SCIP provider 默认关闭，开启 `ossAnalyzerEnrichmentEnabled` 后只读取显式 `scipIndexPath` 或仓库根目录 `index.scip`；文件不存在是 skipped，不是失败。SCIP metadata 必须提供 `tool_info.version`，用于 `AnalyzerRunMetadata` provenance stamp。具体 Java/Python/TypeScript/Rust 等 indexer 的安装、执行和超时控制属于 provider 层，必须继续遵守上述 allowlist、provenance、非阻塞和大仓基准规则。

SCIP 路径的大仓 smoke 用 `scripts/smoke-oss-analyzer-scip.sh`。它只接受已经存在的 `index.scip`，或通过 `--make-scip` 在脚本层运行外部开源 indexer 生成 `index.scip`；AKA runtime 仍只读取、校验、合并和索引 SCIP 结果，不把 SCIP indexer 变成运行时子进程。推荐先在 Apache Dubbo 这类 50 万行以上仓库执行：

```bash
scripts/smoke-oss-analyzer-scip.sh \
  --repo /path/to/dubbo \
  --scip-index /path/to/dubbo/index.scip \
  --query Service \
  --context Service
```

判定标准：baseline analyze 必须先报告 ready；optional SCIP provider 必须明确输出 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 必须返回非空；指定 `--context` 时 definitions 必须非空。脚本默认要求 `--min-lines` 达标；小仓只能显式 `--allow-small-repo` 用于 importer 调试，不能作为大仓基准结论。

`ossAnalyzerFactsPath` 是外部 OSS analyzer adapter 的通用导入入口：文件可以是 `FactBatch` JSON、`FactRecord[]` JSON、一行一个 `FactRecord` 的 JSONL，或带顶层 analyzer 元数据的 bundle：

```json
{
  "analyzer": {
    "analyzerId": "pyright",
    "toolVersion": "1.1.400"
  },
  "stats": { "files": 0, "nodes": 1, "edges": 0, "chunks": 0 },
  "nodes": [],
  "edges": [],
  "chunks": []
}
```

带顶层 `analyzer` 的 bundle 由 AKA runtime 统一 stamp `source` / `provenance` 后再校验；不带顶层 analyzer 的旧格式仍要求每个新增 node/edge 自带完整 provenance。这个入口只消费已经由 rust-analyzer / pyright / jdtls / typescript-language-server / gopls / stack-graphs 等开源工具 adapter 产出的 `aka-facts`；AKA runtime 不在这里启动语言服务、不扫描源码、不做业务语义推断。文件不存在是 skipped；文件内每个新增 node/edge 仍必须通过 allowlist provenance 校验。它不是旧 sidecar、fallback 或 debug transport。

源码调试时可以用内部 runtime 命令 `aka validate-facts <path>` 预检 adapter 输出；这只做合同校验，不写 graph/search，也不是用户可见产品形态。

tree-sitter stack-graphs 路径的大仓 smoke 用 `scripts/oss-analyzer-stack-graphs-python.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-stack-graphs-python.sh` 导入。这个 adapter 是脚本层工具：它启动外部开源 `tree-sitter-stack-graphs-python`，调用官方 `index` / `match` / `query definition` 命令读取 stack-graphs 分析结果，转换成 `File` / `Symbol` / `Reference` 节点、`CONTAINS` / `DEFINES` / `REFERS_TO` 边和 evidence chunks。AKA runtime 仍只读取 `ossAnalyzerFactsPath`、校验 provenance、在 baseline ready 后 staging merge；它不启动 stack-graphs、不持有 analyzer 进程、不扫描源码做自研语义推断。

推荐在 CPython 这类 50 万行以上 Python 仓库执行：

```bash
scripts/smoke-oss-analyzer-stack-graphs-python.sh \
  --repo /path/to/cpython \
  --facts /path/to/cpython/.aka/stack-graphs-python-oss-analyzer-facts.json \
  --tool tree-sitter-stack-graphs-python \
  --tool-version tree-sitter-stack-graphs-python-0.3.0 \
  --max-query-positions 10000 \
  --query-timeout-secs 5 \
  --max-query-timeouts-per-file 2 \
  --query importlib \
  --context main
```

判定标准同 SCIP：Python 源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。stack-graphs 只是外部分析结果来源，不是 runtime fallback/debug channel。adapter 会跳过非 UTF-8 / analyzer 无法解析的单文件；definition query 有单批 timeout 和 per-file timeout 熔断，避免 CPython `_pydecimal.py` 这类慢点拖住整个 enrichment。

Pyright 路径的大仓 smoke 用 `scripts/oss-analyzer-pyright-lsp.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-pyright.sh` 导入。这个 adapter 是脚本层工具：它启动外部开源 `pyright-langserver --stdio`，通过 LSP `textDocument/documentSymbol` 读取 Pyright 的分析结果，转换成 `File` / symbol 节点、`DEFINES` / `CONTAINS` 边和 symbol chunks。AKA runtime 仍只读取 `ossAnalyzerFactsPath`、校验 provenance、在 baseline ready 后 staging merge；它不启动 Pyright、不持有 LSP 会话、不扫描源码做自研语义推断。

推荐在 CPython 这类 50 万行以上仓库执行：

```bash
scripts/smoke-oss-analyzer-pyright.sh \
  --repo /path/to/cpython \
  --facts /path/to/cpython/.aka/pyright-oss-analyzer-facts.json \
  --server 'npx --yes --package pyright@latest pyright-langserver --stdio' \
  --tool-version npx-pyright-latest \
  --query importlib \
  --context main
```

判定标准同 SCIP：源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。

gopls 路径的大仓 smoke 用 `scripts/oss-analyzer-gopls-lsp.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-gopls.sh` 导入。这个 adapter 同样只存在于脚本层：它启动外部开源 `gopls serve`，通过 LSP `textDocument/documentSymbol` 读取 gopls 的分析结果，转换成 `File` / symbol 节点、`DEFINES` / `CONTAINS` 边和 symbol chunks。AKA runtime 不启动 gopls，只读取 adapter 产出的 `aka-facts` bundle。

推荐在 Kubernetes 这类 50 万行以上 Go 仓库执行：

```bash
scripts/smoke-oss-analyzer-gopls.sh \
  --repo /path/to/kubernetes \
  --facts /path/to/kubernetes/.aka/gopls-oss-analyzer-facts.json \
  --server 'gopls serve' \
  --tool-version gopls-v0.22.0 \
  --query kubelet \
  --context main
```

判定标准同 Pyright：Go 源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。

TypeScript 路径的大仓 smoke 用 `scripts/oss-analyzer-typescript-lsp.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-typescript.sh` 导入。adapter 启动外部开源 `typescript-language-server --stdio`（底层 tsserver），通过 LSP `textDocument/documentSymbol` 读取结果，转换成 `File` / symbol 节点、`DEFINES` / `CONTAINS` 边和 symbol chunks。AKA runtime 不启动 tsserver，只读取 adapter 产出的 `aka-facts` bundle。脚本默认跳过 `tests/baselines/reference` 这类生成基线目录，避免语言服务在极端生成文件上崩栈；行数门槛按过滤后的实际 TS/JS 文件重新计算，必须仍达到 `--min-lines`。

推荐在 Microsoft TypeScript 这类 50 万行以上 TS/JS 仓库执行：

```bash
scripts/smoke-oss-analyzer-typescript.sh \
  --repo /path/to/TypeScript \
  --facts /path/to/TypeScript/.aka/typescript-oss-analyzer-facts.json \
  --server 'npx --yes --package typescript-language-server@latest --package typescript@latest typescript-language-server --stdio' \
  --tool-version typescript-language-server-5.3.0 \
  --query Program \
  --context createProgram
```

判定标准同 Pyright/gopls：过滤后的 TS/JS 源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。

rust-analyzer 路径的大仓 smoke 用 `scripts/oss-analyzer-rust-analyzer-lsp.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-rust-analyzer.sh` 导入。adapter 启动外部开源 `rust-analyzer`，通过 LSP `textDocument/documentSymbol` 读取结果，转换成 `File` / symbol 节点、`DEFINES` / `CONTAINS` 边和 symbol chunks。AKA runtime 不启动 rust-analyzer，只读取 adapter 产出的 `aka-facts` bundle。

推荐在 rust-lang/rust 这类 50 万行以上 Rust 仓库执行：

```bash
scripts/smoke-oss-analyzer-rust-analyzer.sh \
  --repo /path/to/rust \
  --facts /path/to/rust/.aka/rust-analyzer-oss-analyzer-facts.json \
  --server 'rustup run stable rust-analyzer' \
  --tool-version rust-analyzer-stable \
  --query rustc \
  --context main
```

判定标准同其它 LSP：Rust 源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。

JDTLS 路径的大仓 smoke 用 `scripts/oss-analyzer-jdtls-lsp.mjs` 生成 bundle，再用 `scripts/smoke-oss-analyzer-jdtls.sh` 导入。adapter 启动外部开源 Eclipse JDT Language Server，通过 LSP `textDocument/documentSymbol` 读取结果，转换成 `File` / symbol 节点、`DEFINES` / `CONTAINS` 边和 symbol chunks。AKA runtime 不启动 JDTLS，只读取 adapter 产出的 `aka-facts` bundle。

推荐在 Apache Cassandra 这类 50 万行以上 Java 仓库执行：

```bash
scripts/smoke-oss-analyzer-jdtls.sh \
  --repo /path/to/cassandra \
  --facts /path/to/cassandra/.aka/jdtls-oss-analyzer-facts.json \
  --server 'jdtls -data /tmp/aka-jdtls-workspace' \
  --tool-version jdtls-2026-06-26 \
  --query Cassandra \
  --context StorageService
```

判定标准同其它 LSP：过滤后的 Java 源码行数必须达到 `--min-lines`，facts bundle 必须先通过 `validate-facts`，baseline graph/search 必须先 ready，`provider=aka-facts-file` 必须明确 merged/skipped/timeout outcome；provider failed、invalid provenance 或 merge failed 视为失败；`search` 和指定 `context` 必须返回非空。脚本默认跳过 `framework-docs`、`test/simulator`、`test/unit`、`tools/stress` 这类会让 JDTLS 长时间卡在单文件符号请求上的目录；行数门槛按过滤后的实际 Java 文件重新计算。

内部 runtime 的 optional enrichment merge 只在 baseline graph/search ready 后追加 facts：新节点和对应 chunks 追加到 search，边写入 graph 并依赖 provenance edge id 去重。merge 使用同一个 `ossAnalyzerEnrichmentMaxSecs` deadline，并先写入临时 staging 副本；只有 merge 全流程成功后才安装回正式 graph/search。merge 失败或 provider 失败只能产生 skipped outcome 与日志，并继续尝试后续 provider；原 baseline graph/search 不被污染、不置 failed、不阻塞查询。

不再新增 Rust 侧自研业务语义 synthesis/enrichment 阶段。Route/GraphQL/Tool/Command/Config/Topic/Table/Migration/Transaction/Process/Community 等增强语义只能来自上述成熟外部事实源；缺失表示 coverage unknown，不能用阻塞式启发算法补齐。旧 AKA engine 内部自研增强 pass 不属于 embedded/direct baseline。

## Progress events

新热路径使用 facts 阶段名：

```json
{"event":"phase","phase":"aka-engine:facts:discover","current":0,"total":0}
{"event":"phase","phase":"aka-engine:facts:parse","current":10,"total":100}
{"event":"phase","phase":"aka-core:fuse-facts:graph","current":0,"total":0}
{"event":"phase","phase":"index:graph:nodes","current":0,"total":0}
{"event":"done","stats":{"files":0,"nodes":0,"edges":0,"chunks":0}}
```

不再定义 `aka-engine:export-artifacts:*` 运行阶段。UI 应把 facts / index / optional OSS analyzer enrichment 分开展示，不再把多个语义阶段折叠成一个 “artifacts” 阶段。Optional OSS analyzer enrichment 的 skipped/timeout 结果不得把仓库状态改成 failed。

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
