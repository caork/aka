# engine/

aka 的解析入口已经切换为原生 `codebase-memory-mcp` 二进制。Rust 侧仍消费
`docs/contracts/artifacts.md` 中的 NDJSON 工件合同，`crates/aka-core` 会运行
CBM CLI、读取它的 SQLite 图谱，再导出 aka 工件。

初始化/构建方式：

```bash
scripts/sync-engine.sh
```

脚本默认使用 aka 维护的 fork `caork/codebase-memory-mcp`，并构建被 git 忽略的
`engine/codebase-memory-mcp-src/` 当前 checkout，复制可执行文件到
`engine/codebase-memory-mcp`，并把当前 engine commit 写入 `engine/ENGINE_SHA`。

解析能力改动直接在 `engine/codebase-memory-mcp-src/` 里做，并提交到
`caork/codebase-memory-mcp`。日常不要维护 aka 仓库内的 patch 堆；只有月度或显式
上游同步时，才用 `scripts/sync-engine.sh --refresh-upstream` 抓取 `aka` fork 和 `upstream`，
随后手工 merge/rebase/cherry-pick 选择性吸收上游 feature。脚本不会 reset 或 clean
维护中的 checkout。不要把大体积源码或构建产物塞进本仓库。
