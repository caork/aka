#!/usr/bin/env bash
# 从 fork 的 engine/aka 分支同步解析引擎到 engine/
# 用法: scripts/sync-engine.sh [fork工作树路径]，默认 ~/Documents/github/GitNexus-engine
set -euo pipefail
SRC="${1:-$HOME/Documents/github/GitNexus-engine}"
DST="$(cd "$(dirname "$0")/.." && pwd)/engine"
echo "TODO(M0): 拷贝清单待 emit.ts 落地后定稿——预计包含:"
echo "  gitnexus/src/core/{ingestion,tree-sitter,graph} gitnexus/src/{lib,config,types}"
echo "  gitnexus-shared/src vendor/ package.json"
echo "  并写入 ENGINE_SHA=$(git -C "$SRC" rev-parse HEAD 2>/dev/null || echo unknown)"
exit 1
