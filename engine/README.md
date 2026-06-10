# engine/

继承自 GitNexus 的 TypeScript 解析管线，经 `scripts/sync-engine.sh` 从
caork/GitNexus 的 `engine/aka` 分支（上游 v1.6.7 基底 + Ascend C 移植）同步到此目录。

本目录内容视为 vendored 代码：**不要在这里直接改 parser 逻辑**——改动应提交到
fork 的 `engine/aka` 分支再同步过来，保证可持续 rebase 上游。
唯一的 aka 原生模块是 `emit.ts`（工件序列化层，合同见 docs/contracts/artifacts.md）。

同步状态：尚未首次同步（M0 进行中），来源 SHA 见 ENGINE_SHA。
