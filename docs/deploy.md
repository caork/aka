# 部署（Docker）

> 许可提醒：解析引擎 `codebase-memory-mcp` 为 MIT，aka 镜像 OCI label 使用
> `org.opencontainers.image.licenses=MIT`。

镜像内容：`aka` 二进制（serve / analyze / mcp 等八命令）+ `codebase-memory-mcp` 原生 C 解析引擎 + git（git 导入用）。CBM 负责解析并写入 SQLite，aka-core adapter 导出 NDJSON 后进入 Rust 索引。默认启动 `aka serve`，HTTP API 在 4111。

## 构建

```bash
# 仓库根目录（Dockerfile 会拉取/构建 codebase-memory-mcp native engine）
docker build -t aka:0.1.0 .
```

多阶段构成：

| 阶段 | 作用 |
|---|---|
| `rust-builder` | `cargo build --release -p aka-cli`（native arch） |
| `rust-cross` | 可选：`--target rust-cross` 单独构建 x86_64 linux 二进制（不在默认链路） |
| `engine-builder` | Debian build 环境里拉取 `codebase-memory-mcp` 并 `make -f Makefile.cbm cbm` |
| `runtime` | debian-slim + git + aka + CBM native engine，非 root（uid 10001），`/data` 数据卷 |

构建慢点正常：rust release 编译 + CBM native engine 编译合计数分钟到十几分钟量级。

## 运行

```bash
# 方式一：compose（推荐）
docker compose up -d

# 方式二：裸 docker
docker run -d --name aka -p 127.0.0.1:4111:4111 -v aka-data:/data aka:0.1.0

curl http://127.0.0.1:4111/api/health     # → {"status":"ok","service":"aka-server"}
```

关键环境变量（镜像内已设好，一般不用动）：

- `AKA_HOME=/data` — registry.json 与各仓库索引数据（artifact / graph.db / search）都在这，**挂卷持久化**。
- `AKA_ENGINE_DIR=/opt/aka/engine` — CBM native engine 目录（含 `codebase-memory-mcp`）。
- `AKA_CBM_MODE=fast|moderate|full` — CBM 解析模式，默认 `fast`。

## 导入仓库

容器内 `analyze` 只能看到容器文件系统，两条路：

```bash
# 路 A：HTTP import（git 来源，容器内 clone 到 /data/checkouts 下）
curl -X POST http://127.0.0.1:4111/api/repos/import \
  -H 'content-type: application/json' \
  -d '{"kind":"git","url":"https://github.com/user/repo.git"}'
# 202 后轮询进度：
curl http://127.0.0.1:4111/api/repos

# 路 B：挂载本地代码 + docker exec
docker run -d --name aka -p 127.0.0.1:4111:4111 \
  -v aka-data:/data -v /path/to/your/repo:/mnt/repo:ro aka:0.1.0
docker exec aka aka analyze /mnt/repo

# zip 上传也行（≤200MB）：
curl -X POST http://127.0.0.1:4111/api/repos/import-zip \
  -F name=myrepo -F file=@repo.zip
```

镜像自带冒烟样本 `/opt/aka/fixtures-demo`（fixtures/demo-ts），可快速验证 CBM engine 和 adapter 在容器内可用：

```bash
docker exec aka aka analyze /opt/aka/fixtures-demo
curl -X POST http://127.0.0.1:4111/api/query \
  -H 'content-type: application/json' -d '{"query":"UserService"}'
```

## 数据卷

`/data`（`AKA_HOME`）布局：`registry.json` + `repos/<slug>-<hash8>/{artifact,graph.db,search}/` +
git 导入的 `checkouts/`。删除容器不丢索引；要全清就 `docker volume rm aka-data`。
容器内用户是非 root（uid 10001），bind mount 目录注意可写权限（git 导入需写 /data）。

## 远程访问注意

- compose / 示例命令默认 **只绑 127.0.0.1**，服务本身无认证。
- **CORS 仅放行 localhost** 来源（aka-server 写死），浏览器从远程域名直接跨域调 API 会被拒。
- 远程模式（认证、反代、桌面端连远端）是 M4 后续项；当前要远程用，建议 SSH 隧道：
  `ssh -L 4111:127.0.0.1:4111 user@host`，别直接把 4111 暴露公网。

## CI 构建与分发（正式渠道）

**镜像不在 macOS 本机构建**（约定）。推送 `v*` tag 触发 `.github/workflows/release.yml`：

1. Dockerfile 按 CBM pin 拉取 `DeusData/codebase-memory-mcp` 并构建 native C binary；
2. 构建 linux/amd64 镜像并**容器内冒烟**（health → `analyze /opt/aka/fixtures-demo` → query 非空，
   覆盖 CBM native engine + SQLite->NDJSON adapter 这一关键风险点）；
3. 推 `ghcr.io/caork/aka:<版本>` 与 `:latest`（私有 package，拉取需 `read:packages` 的 PAT：
   `echo $PAT | docker login ghcr.io -u caork --password-stdin`）；
4. `docker save` 的镜像 tar、Linux/macOS/Windows CLI 二进制、macOS/Windows 桌面 GUI 包、
   Claude Code/OpenCode 插件包、`clients/` 总包与 `SHA256SUMS` 挂到同名 GitHub Release——Jensen 等目标机离线
   `docker load -i aka-<版本>-linux-amd64.docker.tar.gz` 即用。

```bash
# 目标机两种取用方式
docker pull ghcr.io/caork/aka:0.1.0                      # 走 GHCR（需登录）
docker load -i aka-0.1.0-linux-amd64.docker.tar.gz       # 走 release 资产（离线）
```

发行资产：

- `aka-<版本>-x86_64-unknown-linux-gnu.tar.gz` — Linux `aka`
- `aka-<版本>-aarch64-apple-darwin.tar.gz` — macOS `aka`
- `aka-<版本>-x86_64-pc-windows-msvc.zip` — Windows `aka.exe`
- `aka-desktop-<版本>-aarch64-apple-darwin.dmg` — macOS GUI 安装镜像（桌面端更新检查优先展示）
- `aka-desktop-<版本>-aarch64-apple-darwin.app.zip` — macOS GUI（zip 内是 `aka.app`）
- `aka-desktop-<版本>-x86_64-pc-windows-msvc-setup.exe` — Windows GUI 安装包
- `aka-desktop-<版本>-x86_64-pc-windows-msvc-portable.zip` — Windows GUI 免安装包
- `latest.json` — 桌面端更新清单，可同步到 `https://aka.hawkingrad.com/releases/latest.json`

桌面端 Settings → Updates 会读取 hawkingrad 的 `latest.json`。CI 在 tag release 的 checksums 阶段通过
`scripts/release-manifest.mjs` 生成该文件，至少包含 `schemaVersion`、`version`/`latestVersion`、
`releaseUrl`、`publishedAt`/`pub_date`、`downloads` 与 `assets[]`。`downloads` 按
`downloads.macos.dmg`、`downloads.windows.exe` 组织，`assets[]` 保留扁平列表兼容旧客户端；每个资产包含
`platform`、`kind`、`name`、`url`/`downloadUrl`、`size` 与可选 `sha256`。`SHA256SUMS` 只覆盖发布资产，
不包含 `SHA256SUMS` 自身和 `latest.json`。

桌面包必须内置 native `codebase-memory-mcp` engine。`scripts/package-release.sh` 会在准备 Tauri 资源、
生成 macOS `.app.zip`、生成 Windows portable zip 前后校验 `engine/codebase-memory-mcp` 或
`engine/codebase-memory-mcp.exe`，避免把旧的 JS/node engine 目录打进包里却没有 native binary。

`aka-<版本>-...` 裸二进制是 CLI/server 包，能启动 `aka serve` / `aka mcp` / 查询既有索引，但不会打开 GUI；
桌面窗口请下载 `aka-desktop-<版本>-...`。完整 `aka analyze` 需要 `codebase-memory-mcp` 原生二进制：
可以使用 Docker/桌面包内置资源，或设置 `AKA_ENGINE_DIR` 指向含二进制的 engine 目录，或设置 `AKA_CBM_BIN` 指向二进制。

arm64 镜像暂不预构建（engine 阶段需在目标架构编译 CBM native binary，QEMU 全模拟太慢）；
需要时在 arm64 机器上 `docker build -t aka:0.1.0 .` 即可，Dockerfile 全程架构无关。
macOS 本机 CLI/GUI 包由 `scripts/package-release.sh` 产出（不含 Docker）。
