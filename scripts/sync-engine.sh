#!/usr/bin/env bash
# 从 fork 的 engine/aka 分支同步解析引擎到 engine/
# 用法: scripts/sync-engine.sh [fork工作树路径]，默认 ~/Documents/github/GitNexus-engine
#
# 同步内容（rsync --delete，排除 node_modules/dist）：
#   gitnexus/{src,scripts,vendor} + package.json/package-lock.json/tsconfig.json/vitest.config.ts
#   gitnexus-shared/src           + package.json/package-lock.json/tsconfig.json
# 并把来源工作树当前 HEAD（engine/aka 分支）写入 engine/ENGINE_SHA。
#
# 同步后首次使用：
#   cd engine/gitnexus-shared && npm install && npm run build
#   cd engine/gitnexus        && npm install   # postinstall 编译 vendor 语法，prepare 产出 dist（worker 必需）
#   npx tsx src/export/emit-cli.ts --repo <repo> --out <dir>
set -euo pipefail

SRC="${1:-$HOME/Documents/github/GitNexus-engine}"
DST="$(cd "$(dirname "$0")/.." && pwd)/engine"

if [[ ! -d "$SRC/gitnexus/src" || ! -d "$SRC/gitnexus-shared/src" ]]; then
  echo "error: $SRC 不是 GitNexus 工作树（缺 gitnexus/src 或 gitnexus-shared/src）" >&2
  exit 1
fi

SHA="$(git -C "$SRC" rev-parse HEAD)"
mkdir -p "$DST/gitnexus" "$DST/gitnexus-shared"

RSYNC_OPTS=(-a --delete --exclude node_modules --exclude dist)

for d in src scripts vendor; do
  rsync "${RSYNC_OPTS[@]}" "$SRC/gitnexus/$d/" "$DST/gitnexus/$d/"
done
for f in package.json package-lock.json tsconfig.json vitest.config.ts; do
  cp "$SRC/gitnexus/$f" "$DST/gitnexus/$f"
done

rsync "${RSYNC_OPTS[@]}" "$SRC/gitnexus-shared/src/" "$DST/gitnexus-shared/src/"
for f in package.json package-lock.json tsconfig.json; do
  cp "$SRC/gitnexus-shared/$f" "$DST/gitnexus-shared/$f"
done

printf '%s\n' "$SHA" > "$DST/ENGINE_SHA"
echo "synced $SRC -> $DST"
echo "ENGINE_SHA=$SHA"
