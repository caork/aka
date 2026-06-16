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

`label` 取值来自 CBM graph，经 adapter 规范化为 aka 节点标签（Project/Package/Folder/File/Module/Class/Function/Method/Interface/Enum/Type/Route/GraphQL/Tool/Command/Config/Topic/Channel/Table/Repository/Migration/Resource/Transaction/…）。`properties` 为开放对象，aka 侧只索引已知字段（name/filePath/startLine/endLine/…），其余落到 JSON 列存底。

**行号语义**：工件中的 `startLine`/`endLine`（节点与 chunk 同）由 CBM/tree-sitter 坐标导出，合同内保持 **0-based row**。Rust 摄取层（aka-graph/aka-search）写索引时统一 **+1 转为 1-based 人类行号**（`NodeRec::start_line_1based`），因此 SQLite/tantivy 及一切下游（HTTP/MCP/桌面端）的行号都与编辑器、`/api/source` 对齐。`properties` JSON 列存底里保留的是工件原始 0-based 值。

**合成 Community**：若 CBM SQLite 已经产出 `label = "Community"` 节点，adapter 原样透传，不重复合成；若没有，adapter 会按源码模块路径初始化社区，并用 `CALLS` 图做保守标签传播，合成 GitNexus-like `label = "Community"` 社区节点。合成节点的 `id` 形如 `community:heuristic:<hash>`，`properties` 至少包含 `name`、`heuristicLabel`、`cohesion`、`symbolCount`、`keywords`、`enrichedBy = "heuristic"`、`source = "aka-cbm-synth"`。`cohesion` 为 0..1 的启发式内聚度；`symbolCount` 为社区内符号数。

**合成 Process**：若 CBM SQLite 已经产出 `label = "Process"` 节点，adapter 原样透传，不重复合成；若没有，adapter 会基于 `CALLS` 边保守合成 `label = "Process"` 的调用链流程节点。合成流程使用 GitNexus-like 入口评分（调用比、导出/公开、入口命名、框架路径、测试/工具降权）、BFS trace、子链去重、入口-终点去重和动态流程上限。合成节点的 `id` 形如 `process:call-chain:<hash>`，`properties` 至少包含 `name`、`heuristicLabel`、`processType`、`communities`、`communityIds`、`communityLabels`、`trace`、`stepCount`、`entryPointId`、`terminalId`、`source = "aka-cbm-synth"`。其中 `processType` 取 `intra_community` 或 `cross_community`，分别表示流程步骤落在单个社区或跨多个社区；`communities` 为流程涉及的 Community 引用数组（元素至少包含 `id`、`label`）。这是合同内只增字段/节点类型，下游按普通节点摄取。

**Route/GraphQL/Tool/Command 应用语义节点**：`label = "Route"` 表示可被路由工具消费的 API/HTTP route 节点；`properties.name` 通常为 route path，`filePath` 指向 route 定义或 handler 文件，`middleware`、`responseKeys`、`errorKeys` 为可选数组。`label = "GraphQL"` 表示 GraphQL query/mutation/subscription/field 入口；`properties.name`/`operationName` 为 operation 名，`operationType` 为 query/mutation/subscription/field，`handlerId` 指向 resolver/handler。`label = "Tool"` 表示 MCP/RPC/agent tool 定义节点；`properties.name` 为工具名，`filePath` 指向定义文件，`description` 为可选说明。`label = "Command"` 表示 CLI/management command 入口，例如 Spring `CommandLineRunner`/`ApplicationRunner`、picocli `@Command`、Django management command、Click/Typer/argparse；`properties.name` 为命令名，`commandType` 表示命令框架，`handlerId`/`handlerName` 指向处理符号。aka 不要求所有语言/框架都能产出这些节点；缺失时相关工具会返回空结果或显式提示。

**Config 配置节点**：`label = "Config"` 表示 Spring `application.yml/properties`、`@Value`、`@ConfigurationProperties`、Python settings、Django settings、`os.getenv`/`os.environ` 等配置键；`properties.key` 为规范化 key，`configType` 表示 spring-property/env-var/python-setting 等类型，`valueHint`、`sources` 为可选摘要字段。配置文件即使没有 CBM symbol 节点也可由 adapter 直接扫描合成。

**Transaction 事务边界节点**：`label = "Transaction"` 表示业务方法/函数上的事务边界，例如 Spring `@Transactional` 或 Django `transaction.atomic`；`properties.name` 为边界名，`manager` 表示事务管理器来源，`propagation`、`isolation`、`readOnly` 为可选属性。

**Topic/Channel 消息语义节点**：`label = "Topic"` 表示 Kafka/RabbitMQ/JMS/SQS/NATS/STOMP/Socket.IO/EventEmitter/Django Channels/WebSocket 等消息主题、队列或实时通道；`properties.name` 为 topic/queue/channel 名，`broker` 为 transport/broker，`consumerGroups` 为可选消费组数组，`topicSource` 标识来源（例如 `source-scan`、`native-channel` 或组合）。CBM 原生 `label = "Channel"` 节点也会原样透传；adapter 同时把原生 `Channel` + `EMITS`/`LISTENS_ON` 结构化事实桥接成 `Topic` + `PUBLISHES_TOPIC`/`CONSUMES_TOPIC`，以便搜索、图和 impact 使用统一 Topic 语义。桥接边 evidence 会保留 `source = "codebase-memory-mcp"`、`nativeLabel = "Channel"`、`nativeEdgeType = "EMITS" | "LISTENS_ON"`。

**Persistence/Migration 节点**：`label = "Table"` / `"Repository"` 表示由 JPA/SQLAlchemy/Django 等实体和 repository 推导出的持久化语义；`properties.tableName`、`entityName`、`columns`、`repositorySource` 等为可选属性。`label = "Migration"` 表示 Flyway/Liquibase/Alembic/Django migration 等 schema 变更脚本；`properties.migrationType`、`version`、`tables`、`operations` 描述迁移框架、版本、涉及表和 create/alter/drop 等操作。若 migration 涉及的表没有 ORM entity，adapter 可合成 `tableSource = "migration-script"` 的 Table。adapter 还会从高置信 SQL 字符串、Spring Data `@Query`、SQLAlchemy/Django ORM 实体查询中合成方法/函数到 Table 的读写边。

### edges.ndjson — aka contract `GraphRelationship`

```json
{"id":"...","sourceId":"...","targetId":"...","type":"CALLS","confidence":0.9,"reason":"...","step":1,"evidence":[{"kind":"...","weight":0.5}]}
```

`type` 取值来自 CBM graph，经 adapter 规范化为 aka 关系类型（CONTAINS/DEFINES/CALLS/IMPORTS/INHERITS/IMPLEMENTS/DEPENDS_ON/HTTP_CALLS/ACCESSES_RESOURCE/FETCHES/HANDLES_ROUTE/HANDLES_GRAPHQL/HANDLES_TOOL/HANDLES_COMMAND/HANDLES_JOB/ENQUEUES_JOB/USES_STEP/USES_CONFIG/CONSUMES_TOPIC/PUBLISHES_TOPIC/HAS_TRANSACTION_BOUNDARY/MAPS_TO_TABLE/REPOSITORY_FOR/MIGRATES_TABLE/READS_TABLE/WRITES_TABLE/READS/WRITES/…）。`step`/`evidence` 可选，缺失时下游按普通图边处理。

应用语义相关边的当前消费语义：

- `HANDLES_ROUTE`：handler 符号/文件 → Route。`route_map` 用它找 route handler；没有时回退 Route 自身的 `filePath`。
- `FETCHES`：前端/客户端消费者 → Route。`route_map`、`shape_check`、`api_impact` 用它列直接消费者；当前工具从边 `reason` 的 `keys:<a,b>|fetches:<n>` 摘要解析 consumer `accessedKeys` 和可选 `fetchCount`，adapter 也可在 `evidence` 中保留同名原始字段。`HTTP_CALLS` 也会作为兼容的 route consumer 边消费。
- `ACCESSES_RESOURCE`：业务方法/函数 → Resource。用于 S3/GCS/Azure Blob/Object storage 等非 HTTP 外部资源访问；query/context/impact 会按普通图边消费。
- `HANDLES_GRAPHQL`：resolver/handler 符号 → GraphQL operation。`graphql_map` 用它列 resolver handlers；query/context/impact 也会按普通定义节点和图边消费。
- `HANDLES_TOOL`：handler 符号/文件 → Tool。`tool_map` 用它列工具处理函数。
- `HANDLES_COMMAND`：handler 符号/类 → Command。query/context/impact 会按普通定义节点和图边消费，用于识别运维脚本、管理命令和 CLI 入口的业务影响面。
- `HANDLES_JOB` / `ENQUEUES_JOB`：Job handler → Job、业务方法/函数 → Job。query/context/impact 会按普通图边消费，用于识别 Spring scheduled/async、Celery/RQ/APScheduler 等后台任务及其触发者。
- `USES_STEP`：Spring Batch Job → Step handler。query/context/impact 会按普通图边消费，用于识别批处理 Job 对 Step Bean 的编排依赖。
- `DEPENDS_ON`：业务类/方法/函数 → 被依赖的类/接口/函数。adapter 从项目源码事实中合成，例如 Spring 构造/字段注入、`@Bean` 参数、FastAPI `Depends`/`Security`；测试源码会按 git/project source set 和构建配置声明的 test roots 排除。
- `USES_CONFIG`：业务方法/类/模块 → Config。query/context/impact 会按普通图边消费，用于定位配置键、环境变量和 settings 变更影响的代码。
- `CONSUMES_TOPIC` / `PUBLISHES_TOPIC`：业务方法/函数/文件 → Topic。query/context/impact 会按普通图边消费，用于定位消息消费者、发布者和主题变更影响。adapter 可从源码扫描合成，也可从 CBM 原生 `Channel` + `LISTENS_ON`/`EMITS` 桥接；同一 source node/broker/topic/direction 会去重。
- `HAS_TRANSACTION_BOUNDARY`：业务方法/函数 → Transaction。context/impact 会按普通图边消费，用于识别 Spring/Django 等服务的事务边界。
- `MIGRATES_TABLE`：Migration → Table。query/context/impact 会按普通图边消费，用于把 schema migration 变更映射到表、repository、entity 和业务流程风险面。
- `READS_TABLE` / `WRITES_TABLE`：业务方法/函数 → Table。query/context/impact 会按普通图边消费，用于把 SQL/ORM 查询、写入和 schema 表影响面连接到具体服务代码。

合成 Community 会追加：

- `MEMBER_OF`：符号 → Community，表示符号所属社区。

合成 Process 会追加两类边：

- `ENTRY_POINT_OF`：入口符号 → Process，`step` 为空。
- `STEP_IN_PROCESS`：流程步骤符号 → Process，`step` 为 1-based 步号。

这两类边的 `evidence.source` 为 `aka-cbm-synth`。aka-graph/aka-mcp 使用 `STEP_IN_PROCESS` 展示流程归属、流程步骤和 impact/detect_changes 的 affected_processes；`ENTRY_POINT_OF` 只标注流程入口，不单独算作符号参与某流程的 membership。

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
