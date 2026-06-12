# engine/

aka 的解析入口已经切换为原生 `codebase-memory-mcp` 二进制。Rust 侧仍消费
`docs/contracts/artifacts.md` 中的 NDJSON 工件合同，`crates/aka-core` 会运行
CBM CLI、读取它的 SQLite 图谱，再导出 aka 工件。

同步/构建方式：

```bash
scripts/sync-engine.sh
```

脚本会 clone / build `DeusData/codebase-memory-mcp` 到被 git 忽略的
`engine/codebase-memory-mcp-src/`，复制可执行文件到 `engine/codebase-memory-mcp`，
并把来源 commit 写入 `engine/ENGINE_SHA`。需要本地调 parser 时，请在 CBM
上游或本地 checkout 中改动后再重新运行同步脚本，不要把大体积源码塞进本仓库。
