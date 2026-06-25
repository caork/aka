# 部署（Docker）

> 许可提醒：解析引擎 AKA engine 为 MIT，aka 镜像 OCI label 使用
> `org.opencontainers.image.licenses=MIT`。

镜像内容：`aka` 二进制（serve / analyze / mcp 等内部 runtime 命令）+ embedded AKA engine 原生 C 解析引擎 + git（git 导入用）。索引运行路径是 `aka-facts` direct pipeline：解析器产出 facts，Rust 图/搜索 writer 直接消费；镜像不保留 AKA engine binary、facts sidecar NDJSON 或 engine SQLite->legacy artifact fallback。默认启动 `aka serve`，HTTP API 在 4111。

## 构建

```bash
# 仓库根目录（Dockerfile 会拉取/构建 AKA engine）
docker build -t aka:0.1.0 .
```

多阶段构成：

| 阶段 | 作用 |
|---|---|
| `rust-builder` | `cargo build --release -p aka-cli`（native arch） |
| `rust-cross` | 可选：`--target rust-cross` 单独构建 x86_64 linux 二进制（不在默认链路） |
| `engine-builder` | Debian build 环境里拉取 `aka-engine` 并构建 embedded engine runtime |
| `runtime` | debian-slim + git + aka + embedded AKA engine，非 root（uid 10001），`/data` 数据卷 |

构建慢点正常：rust release 编译 + AKA engine 编译合计数分钟到十几分钟量级。

## 运行

```bash
# 方式一：compose（推荐）
docker compose up -d

# 方式二：裸 docker
docker run -d --name aka -p 127.0.0.1:4111:4111 -v aka-data:/data aka:0.1.0

curl http://127.0.0.1:4111/api/health     # → {"status":"ok","service":"aka-server"}
```

关键环境变量（镜像内已设好，一般不用动）：

- `AKA_HOME=/data` — registry.json 与各仓库索引数据（graph.db / search）都在这，**挂卷持久化**。
- engine runtime — 随镜像编译/链接，不要求配置外置 `aka-engine` 目录。
- `AKA_ENGINE_MODE=fast|moderate|full` — AKA engine 解析模式，默认 `fast`。

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

镜像自带冒烟样本 `/opt/aka/fixtures-demo`（fixtures/demo-ts），可快速验证 embedded AKA engine facts 和 Rust 索引在容器内可用：

```bash
docker exec aka aka analyze /opt/aka/fixtures-demo
curl -X POST http://127.0.0.1:4111/api/query \
  -H 'content-type: application/json' -d '{"query":"UserService"}'
```

## 数据卷

`/data`（`AKA_HOME`）布局：`registry.json` + `repos/<slug>-<hash8>/{graph.db,search}/` +
git 导入的 `checkouts/`。`graph.db` 是 aka-graph 的 SQLite 持久化，不是旧 engine SQLite artifact。删除容器不丢索引；要全清就 `docker volume rm aka-data`。
容器内用户是非 root（uid 10001），bind mount 目录注意可写权限（git 导入需写 /data）。

## 远程访问注意

- compose / 示例命令默认 **只绑 127.0.0.1**，服务本身无认证。
- **CORS 仅放行 localhost** 来源（aka-server 写死），浏览器从远程域名直接跨域调 API 会被拒。
- 远程模式（认证、反代、桌面端连远端）是 M4 后续项；当前要远程用，建议 SSH 隧道：
  `ssh -L 4111:127.0.0.1:4111 user@host`，别直接把 4111 暴露公网。

## CI 构建与分发（正式渠道）

**镜像不在 macOS 本机构建**（约定）。推送 `v*` tag 触发 `.github/workflows/release.yml`：

1. Dockerfile 按 AKA engine pin 拉取 aka 维护的 engine fork 并构建 embedded engine runtime；日常 pin 由 `engine/ENGINE_SHA` 经 `scripts/pin-engine-ref.sh` 同步，只在显式上游验证或临时分支验证时用 `--build-arg AKA_ENGINE_REPO=... --build-arg AKA_ENGINE_REF=...` 覆盖；
2. 构建 linux/amd64 镜像并**容器内冒烟**（health → `analyze /opt/aka/fixtures-demo` → query 非空，
   覆盖 embedded AKA engine facts + Rust graph/search writer 这一关键风险点）；
3. 推 `ghcr.io/caork/aka:<版本>` 与 `:latest`（私有 package，拉取需 `read:packages` 的 PAT：
   `echo $PAT | docker login ghcr.io -u caork --password-stdin`）；
4. `docker save` 的镜像 tar、macOS/Windows 桌面 GUI 包、Claude Code/OpenCode 插件包、
   `clients/` 总包与 `SHA256SUMS` 挂到同名 GitHub Release——Jensen 等目标机离线
   `docker load -i aka-<版本>-linux-amd64.docker.tar.gz` 即用。

```bash
# 目标机两种取用方式
docker pull ghcr.io/caork/aka:0.1.0                      # 走 GHCR（需登录）
docker load -i aka-0.1.0-linux-amd64.docker.tar.gz       # 走 release 资产（离线）
```

发行资产：

- `aka-desktop-<版本>-aarch64-apple-darwin.dmg` — macOS GUI 安装镜像（桌面端更新检查优先展示）
- `aka-desktop-<版本>-aarch64-apple-darwin.app.zip` — macOS GUI（zip 内是 `aka.app`）
- `aka-desktop-<版本>-aarch64-apple-darwin.app.tar.gz` — macOS Tauri updater 包（有 updater 签名密钥时生成）
- `aka-desktop-<版本>-aarch64-apple-darwin.app.tar.gz.sig` — macOS Tauri updater 签名旁路文件
- `aka-desktop-<版本>-macos-open.sh` — macOS 无 Apple Developer ID/无公证包打开助手
- `aka-desktop-<版本>-x86_64-pc-windows-msvc-setup.exe` — Windows GUI 安装包
- `aka-desktop-<版本>-x86_64-pc-windows-msvc-setup.exe.sig` — Windows Tauri updater 签名旁路文件
- `aka-desktop-<版本>-x86_64-pc-windows-msvc-portable.zip` — Windows GUI 免安装包
- `aka-claude-code-plugin-<版本>.zip` — Claude Code 插件包
- `aka-opencode-plugin-<版本>.zip` — OpenCode 本地 plugin + MCP/skill 配置包
- `aka-clients-<版本>.tar.gz` — 全量客户端接入文件
- `aka-<版本>-linux-amd64.docker.tar.gz` — Docker 镜像离线包
- `SHA256SUMS` — 发布资产校验和
- `latest.json` — 桌面端更新清单（默认发布到 GitHub Release 的 `latest/download/latest.json`）

桌面端 Settings → Updates 优先走 Tauri 原生 updater 自动下载安装；若当前构建没有 updater 配置，则回退为读取
GitHub Release 的 `latest.json` 并打开下载链接。CI 在 tag release 的 checksums 阶段通过
`scripts/release-manifest.mjs` 生成该文件，至少包含 `schemaVersion`、`version`/`latestVersion`、
`releaseUrl`、`publishedAt`/`pub_date`、`notes`、`platforms`、`downloads` 与 `assets[]`。

`platforms` 按 Tauri v2 updater 静态 JSON 约定生成，键为 `OS-ARCH`（例如
`darwin-aarch64`、`darwin-x86_64`、`windows-x86_64`），每项只放 updater 所需的
`signature` 与 `url`。`signature` 是同目录旁路签名文件（`.sig`，兼容 `.signature`）的文件内容，不是
签名文件 URL；`url` 指向实际更新包（macOS 为 `.app.tar.gz`，Windows 为 NSIS `setup.exe`）。没有签名旁路
文件的资产不会进入 `platforms`，避免生成 Tauri 无法验证的自动更新项。

`downloads` 与 `assets[]` 保留给现有下载 UI/旧客户端：`downloads` 仍按 `downloads.macos.dmg`、
`downloads.windows.exe` 组织，`assets[]` 保留扁平列表；每个资产包含 `platform`、`kind`、`name`、
`url`/`downloadUrl`、`size`、可选 `sha256`，有签名时还会带 `signatureName`/`signatureUrl`。
`SHA256SUMS` 只覆盖发布资产，不包含 `SHA256SUMS` 自身和 `latest.json`。

Updater 公钥/签名私钥只允许通过 CI secret / 本地环境变量传给 Tauri CLI（`TAURI_UPDATER_PUBKEY`、
`TAURI_SIGNING_PRIVATE_KEY` 与可选 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`），不要写入仓库、dist 文件或文档。
`scripts/package-release.sh` 会在检测到公钥和私钥时临时传入 updater 配置与
`bundle.createUpdaterArtifacts=true`，从 Tauri 产物目录复制 `.app.tar.gz`/`.sig` 或 `setup.exe.sig` 到
`dist/`；没有这些环境变量时仍正常发布手动下载包，只是 `latest.json.platforms` 为空并给出 warning。tag
release 使用 `AKA_TAURI_UPDATER=required` 和 `--require-updater true`，缺 secret 或缺签名平台会直接失败。
默认 updater 端点为 `https://github.com/caork/aka/releases/latest/download/latest.json`；如需切到 CDN 或
hawkingrad mirror，可在 CI/本地打包时设置 `AKA_UPDATER_ENDPOINT` 覆盖。

桌面包必须内置 embedded engine runtime。Windows portable 的用户形态固定为单文件 `AKA.exe`，不把
`engine\aka-engine.exe` 或 `aka_engine.dll` 作为外置文件发布；`AKA.exe` 通过内置 `aka_engine.dll`
驱动 embedded/direct-facts 路径，不再内置 `aka-engine.exe` fallback/debug。发布验证以 Windows 侧启动
`AKA.exe` 后通过 MCP 完成索引构图和查询为准。

Release 不再发布 `aka-<版本>-...` 裸命令行/server 包。用户侧只交付桌面端和插件包；桌面包里的 `AKA`
可执行文件支持 `serve` / `mcp` / `analyze` 等子命令仅用于插件宿主、headless 调试和源码开发。headless
场景使用 Docker 镜像。源码开发时仍可 `cargo build --release -p aka-cli` 得到单独的 `aka` 调试二进制。

arm64 镜像暂不预构建（engine 阶段需在目标架构编译 embedded AKA engine runtime，QEMU 全模拟太慢）；
需要时在 arm64 机器上 `docker build -t aka:0.1.0 .` 即可，Dockerfile 全程架构无关。
macOS 本机桌面包由 `scripts/package-release.sh --desktop-only` 产出（不含 Docker）。

Windows 发包必须在 Windows 侧对发布产物跑完整系统测试，而不是只看 CI 编译或 zip 文件布局。标准 smoke 路径是解压
portable 后运行 `scripts/smoke-windows-portable.ps1`：它启动 `AKA.exe`，等待 `127.0.0.1:4112/mcp`，通过 MCP
调用 `analyze` 索引一个 Spring Java fixture，再调用 `list_repos`、`search_code`、`query`、`context` 验证构图和查询。
平时开发可按改动面选择轻量测试；准备发布 Windows 包时这条是硬门槛。
