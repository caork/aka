# engine/

aka 的解析入口使用第一方原生 engine 的 direct facts API。
唯一运行时合同是 `docs/contracts/artifacts.md` 中的 `aka-facts`：engine/library
直接产出 nodes/edges/chunks facts，Rust graph/search writer 直接消费 `FactSource`。

旧 `aka-engine` binary、engine SQLite、legacy artifact export 和 facts sidecar NDJSON
不再作为 fallback 或调试通道。

初始化/构建方式：

```bash
scripts/sync-engine.sh
```

脚本默认使用 aka 维护的第一方仓库 `caork/aka-engine`，并构建被 git 忽略的
`engine/aka-engine-src/` 当前 checkout，把当前 engine commit 写入 `engine/ENGINE_SHA`。
Windows/MSYS 构建产出 `aka_engine.dll`，供桌面 `AKA.exe` 内置后作为
embedded/direct-facts 运行路径；不再准备或依赖 `aka-engine.exe` fallback。
随后运行 `scripts/pin-engine-ref.sh`，把 Dockerfile 和 release workflow 的 `AKA_ENGINE_REF`
同步到同一个 commit，保证 Docker/Windows/macOS 分发用的都是我们维护并验证过的 engine。

解析能力改动直接在 `engine/aka-engine-src/` 里做，并提交到
`caork/aka-engine`。日常不要维护 aka 仓库内的 patch 堆；只有月度或显式
上游同步时，才用 `scripts/sync-engine.sh --refresh-upstream` 抓取 `aka` fork 和 `upstream`，
随后手工 merge/rebase/cherry-pick 选择性吸收上游 feature。脚本不会 reset 或 clean
维护中的 checkout。不要把大体积源码或构建产物塞进本仓库。
