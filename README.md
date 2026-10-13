# SerialBridge

[![Test, build and package](https://github.com/kiliter/SerialBridge/actions/workflows/check.yml/badge.svg)](https://github.com/kiliter/SerialBridge/actions/workflows/check.yml)
[![Release](https://img.shields.io/github/v/release/kiliter/SerialBridge?label=release)](https://github.com/kiliter/SerialBridge/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

通过 USB 串口连接两台电脑：把本机 TCP 端口透传到对端可访问的地址，也可以把 HTTP 请求、浏览器请求和文件传输交给另一端执行。

> [!WARNING]
> 当前版本为 `0.0.1`，属于早期预发布。软件链路（含 TCP 转发与 Xray/Mihomo VLESS loopback 联调）、三平台构建和真实 Chromium 扩展已通过测试；FT232RL 双机实机链路、网站登录态兼容性、托盘行为以及签名/公证分发尚未验收。详见 [交付与验证状态](SerialBridge-Delivery-Status.md)。

## 目录

- [功能特性](#功能特性)
- [工作原理](#工作原理)
- [安装](#安装)
- [快速开始](#快速开始)
- [TCP 转发](#tcp-转发)
- [使用示例](#使用示例)
- [配置](#配置)
- [从源码构建](#从源码构建)
- [测试](#测试)
- [项目结构](#项目结构)
- [安全模型与已知限制](#安全模型与已知限制)
- [文档](#文档)
- [许可证](#许可证)

## 功能特性

- **对等节点**：两端运行同一个 Core，没有固定的 Host/Client 或 Mac/Win 角色，任一端都可发起请求。
- **本机 HTTP API**：只监听 `127.0.0.1`（默认端口 `9527`），使用 Bearer Token 鉴权。
- **TCP 转发**：本机监听端口，对端连接目标地址，字节原样透传，可承载 VLESS（Xray/Mihomo）、TLS、SSH 等任意 TCP 协议。
- **远端执行**：`http.fetch` 由对端通过 `reqwest` 发起普通 HTTP 请求；`browser.fetch` 由对端已配对的浏览器扩展执行。
- **文件传输**：异步下载任务、拉取对端共享目录文件、推送本地文件，传输完成后校验 SHA-256。
- **桌面客户端**：Tauri 2 应用，内嵌 Core，提供总览、日志、文件传输、TCP 转发、设置五个页面和系统托盘。
- **Headless 模式**：`serialbridge` 命令行程序，适合无界面环境。
- **默认拒绝**：来源白名单默认为空，请求方法默认只允许 `GET` / `HEAD`，重定向一律拒绝。

## 工作原理

```text
本机程序 ──HTTP :9527──▶ Core A ◀══ USB 串口 ══▶ Core B ──┬─ reqwest ──▶ 目标网站
                                                         └─ WebSocket ─▶ 浏览器扩展 ──▶ 目标网站
```

两端通过自定义二进制帧协议 `SBR1` 通信，每帧带 CRC32，支持多个请求并发交错和心跳检测。文件以有界的单块请求/应答方式传输（默认每块 8 KiB）。协议细节见 [docs/PROTOCOL.md](docs/PROTOCOL.md)。

## 安装

从 [GitHub Releases](https://github.com/kiliter/SerialBridge/releases) 下载对应平台的制品（标记为 Latest 的是正式版本，带 `-<sha7>` 后缀的是 `main` 分支的预发布）：

| 平台 | 制品 | 说明 |
|---|---|---|
| Windows x64 | NSIS `.exe` | 需要 WebView2 和 USB 串口驱动 |
| macOS（Apple Silicon / Intel） | universal `.dmg` | 未进行 Developer ID 签名和公证 |
| Linux x64 | `.deb` / `.AppImage` | AppImage 可能需要 FUSE |
| Chrome / Chromium | 扩展 `.zip` | 解压后以「加载已解压的扩展程序」方式安装 |

请用发布页附带的 `SHA256SUMS` 校验下载文件。各平台安装说明见 [docs/RELEASE.md](docs/RELEASE.md)。

## 快速开始

两台电脑各自完成以下步骤，双方波特率必须一致。

1. 安装并启动桌面客户端，或使用 [Headless 模式](#headless-模式)。首次启动会生成配置文件、随机 API Token、下载目录和共享目录。
2. 在「设置」中选择实际串口，配置波特率。
3. 在**执行节点**的「设置」中添加允许访问的精确来源，例如 `https://example.com`。
4. 在**发起节点**读取本机 Token 并调用 API：

```bash
export SERIALBRIDGE_TOKEN="$(serialbridge token)"

curl http://127.0.0.1:9527/v1/health \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN"
```

> [!CAUTION]
> API Token 只用于本机访问，不要发给对端，也不要贴到共享日志里。

### Headless 模式

```bash
serialbridge init      # 初始化配置
serialbridge ports     # 列出系统串口
serialbridge serve     # 启动节点
serialbridge status    # 查询本机节点状态
serialbridge token     # 显示本机 API Token
```

所有子命令都支持 `--config <path>` 指定配置文件。从源码运行时将 `serialbridge` 换成 `cargo run -p serialbridge-core --`。

桌面客户端已内嵌 Core，不要同时运行桌面客户端和 `serve` 占用同一端口或串口。

### 浏览器扩展

使用 `browser.fetch` 或浏览器下载时，执行节点需要安装并配对扩展；发起节点不需要。

1. 在 Chromium 内核浏览器中开启开发者模式，加载解压后的扩展目录（源码构建时为 `browser-extension/dist`）。
2. 在桌面客户端「设置」中点击「生成配对码」。
3. 在扩展弹窗中填写本机 HTTP 端口和 12 位配对码。配对码有效期 180 秒，最多尝试 5 次。
4. 在扩展弹窗中授权目标网站，同时确认执行节点的来源白名单已包含该来源。

扩展会在浏览器规则允许的范围内使用当前登录状态，但不注入页面脚本、不导出 Cookie、不执行任意 JS，扩展离线时也不会自动回退到普通 HTTP。

## TCP 转发

SerialBridge 只负责把 TCP 字节搬过串口，不解析应用协议。下面以 VLESS 为例：B 机部署 Xray 服务端，A 机用 Mihomo 作客户端。

```text
应用 → Mihomo（A） → 127.0.0.1:10443 → SerialBridge A ══ 串口 ══ SerialBridge B → 127.0.0.1:443 → Xray（B） → 目标网站
```

1. 在 B 机正常部署 Xray，例如 VLESS inbound 监听 `127.0.0.1:443`。B 机的 SerialBridge 不需要额外配置。
2. 在 A 机 SerialBridge 的「TCP 转发」页添加规则，或写入配置文件：

```toml
[[forwards]]
listen = "127.0.0.1:10443"   # A 机本地监听地址
target = "127.0.0.1:443"     # B 机视角下的目标地址
```

3. Mihomo 中把节点地址指向 A 机的本地监听端口，其余参数与 Xray 服务端保持一致：

```yaml
proxies:
  - name: serial-xray
    type: vless
    server: 127.0.0.1
    port: 10443
    uuid: <Xray 中配置的 UUID>
    network: tcp
    # tls / reality / flow 与服务端一致
```

规则可以有多条，例如再加一条 `127.0.0.1:2222 → 127.0.0.1:22` 用来转发 SSH。每条连接独立流控；串口断开时所有转发连接会被关闭，由客户端自行重连。吞吐受串口波特率限制（115200 约 11 KB/s，3 Mbaud 理论约 300 KB/s）。

> [!NOTE]
> `target` 由 A 端指定，B 端会连接任何 A 要求的地址。监听地址建议保持 `127.0.0.1`；若监听其他地址，能访问该端口的任何人都可以使用隧道。

## 使用示例

以下命令均在发起节点执行。`target` 为 `peer` 表示由对端执行，为 `local` 表示在本机执行。

**RPC 请求**（方法：`system.ping`、`http.fetch`、`browser.fetch`）：

下面示例将 `params.method` 改为 `HEAD` 可只查询响应头，返回空响应体；目标资源的 `Content-Length` 不计入 16 KiB 内联响应体上限。

```bash
curl -X POST http://127.0.0.1:9527/v1/rpc \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"target":"peer","method":"browser.fetch","params":{"url":"https://example.com/api","method":"GET"}}'
```

**创建下载任务**（`executor` 可为 `http` 或 `browser`）：

```bash
curl -X POST http://127.0.0.1:9527/v1/downloads \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"target":"peer","executor":"browser","url":"https://example.com/report.zip","filename":"report.zip"}'
```

接口返回 `202` 和任务对象。用 `GET /v1/downloads/{id}` 查询进度，完成后从 `GET /v1/downloads/{id}/content` 获取文件。

**拉取对端共享目录中的文件**：

```bash
curl -X POST http://127.0.0.1:9527/v1/files/pull \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"filename":"report.zip"}'
```

**推送本地文件到对端**（请求必须带 `Content-Length`，不支持仅 chunked 的上传）：

```bash
curl -X PUT http://127.0.0.1:9527/v1/files/push/report.zip \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @report.zip
```

完整接口、参数限制和错误码见 [docs/API.md](docs/API.md)。

## 配置

默认配置文件路径为 `~/.serialbridge/config.toml`（Windows 为 `%USERPROFILE%\.serialbridge\config.toml`），可通过 `SERIALBRIDGE_CONFIG` 环境变量或 `--config` 参数覆盖。示例见 [configs/node-a.toml](configs/node-a.toml) 与 [configs/node-b.toml](configs/node-b.toml)，其中串口名需替换为实际值。

| 配置项 | 默认值 | 说明 |
|---|---|---|
| `http_port` | `9527` | 本机 API 端口，修改后需重启 |
| `serial_port` | 空 | 串口名，例如 `COM3` 或 `/dev/tty.usbserial-xxxx` |
| `baud_rate` | `115200` | 范围 9600–3000000，两端必须一致 |
| `allowed_origins` | `[]` | 精确来源白名单，比较协议、主机和端口 |
| `allowed_methods` | `["GET", "HEAD"]` | 允许的 HTTP 方法 |
| `chunk_bytes` | `8192` | 文件分块大小，范围 1024–16384 |
| `max_file_bytes` | 100 MiB | 单文件上限，最大 10 GiB |
| `max_spool_bytes` | 1 GiB | 下载/暂存目录配额 |
| `auto_reconnect` | `true` | 串口断开后自动重连 |
| `forwards` | `[]` | TCP 转发规则，`listen` 为本机 `ip:port`，`target` 为对端视角的 `host:port` |
| `download_dir` / `shared_dir` | `downloads` / `shared` | 下载目录 / 共享目录 |

大部分配置可在「设置」页或通过 `PUT /v1/settings` 修改；目录路径与凭据不能通过 API 修改。

## 从源码构建

### 环境要求

- Node.js 22.8+ 和 npm
- Rust stable
- 桌面构建的平台依赖（参考 [Tauri 官方文档](docs/REFERENCES.md)）：
  - Windows：MSVC 构建工具、WebView2
  - macOS：Xcode Command Line Tools
  - Linux：`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf`

仅构建 Core / Headless 不需要桌面依赖。

### 构建命令

```bash
npm ci                                    # 安装前端依赖
npm run build                             # 构建 UI 与浏览器扩展（不编译 Rust）
cargo build -p serialbridge-core --locked # 构建 Core 与 CLI
npm run desktop:dev                       # 以开发模式运行桌面客户端
npm run desktop:build                     # 构建桌面安装包
npm run extension:package                 # 打包扩展 ZIP 到 artifacts/
```

### 无硬件调试

Core 提供仅限 loopback 的 TCP 传输，用于在同一台机器上运行两个节点（两份配置的 HTTP 端口需不同）：

```bash
cargo run -p serialbridge-core -- --config node-a.toml serve --dev-tcp-listen 127.0.0.1:19727
cargo run -p serialbridge-core -- --config node-b.toml serve --dev-tcp-connect 127.0.0.1:19727
```

独立预览 Web 控制台：执行 `npm run ui:serve` 后访问 `http://127.0.0.1:4173/`，输入本地 API Token 连接 headless 节点。加上 `?demo=1` 可进入带提示的显式演示模式。

## 测试

```bash
npm test                                                        # TypeScript 构建 + Node 测试
cargo fmt --all -- --check
cargo clippy -p serialbridge-core --all-targets --locked -- -D warnings
cargo test -p serialbridge-core --locked                        # Core 单元与集成测试
python tests/e2e.py                                             # 两个真实 headless 进程的端到端流程
```

浏览器相关测试需要 Playwright 和 Chromium：

```bash
python -m pip install -r requirements-test.txt
python -m playwright install chromium
python tests/e2e.py --browser      # 真实 Core + Web 控制台
python tests/e2e.py --extension    # 额外加载真实 MV3 扩展，需允许未打包扩展
python tests/test_packaging.py
```

CI（[`.github/workflows/check.yml`](.github/workflows/check.yml)）在 Windows、macOS、Linux 上运行以上检查，构建安装包并启动原生桌面进程；推送到 `main` 后自动发布预发布版本（tag 形如 `v0.0.1-<sha7>`）；推送与 `package.json` 版本一致的 `v*` tag（如 `v0.0.1`）后自动发布正式版本并标记为 Latest，发布说明取自 `docs/releases/<版本>.md`。测试证据保存在 [docs/verification](docs/verification)。

## 项目结构

```text
.
├── crates/
│   ├── core/              # Rust 节点库与 serialbridge CLI（协议、传输、API、文件、扩展、TCP 转发）
│   └── desktop/           # Tauri 2 桌面入口，内嵌 Core
├── ui/                    # TypeScript Web 控制台（src → dist）
├── browser-extension/     # Chromium MV3 扩展（src/public → dist）
├── tests/                 # Node 测试、Python 端到端与打包测试
├── scripts/               # 构建、打包与源码检查脚本
├── configs/               # 双节点示例配置
└── docs/                  # API、协议、发布、设计与验证记录
```

## 安全模型与已知限制

- API 只监听 loopback，API Token 与扩展凭据相互独立。
- CRC32 和 SHA-256 只保证完整性，不提供身份认证或加密。USB 物理对端被视为受信任节点，请仅在获得授权的设备与网络环境中使用。
- 只支持一个串口对端、一个已配对扩展和一个活动扩展连接。
- TCP 转发只做透传，链路本身不加密；需要保密时请在应用层使用 TLS/REALITY 等。
- 不提供 SOCKS/HTTP 代理入口、远程 shell、脚本执行、任意路径读取或 Cookie 导出。
- 文件传输没有滑动窗口、断点续传、HTTP Range 或串口加密，不适合高速传输大文件。
- 任务、日志保存在内存中，重启后不恢复；已完成的下载文件会保留在磁盘上。
- 单节点最多 2 个主传输任务、2 个暂存上传、64 个等待中的对端 RPC、8 个等待中的浏览器任务，资源繁忙时直接返回 `BUSY`。
- 暂不支持开机自启、自动升级和多浏览器配置。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/API.md](docs/API.md) | HTTP API、参数限制、权限边界与错误码 |
| [docs/PROTOCOL.md](docs/PROTOCOL.md) | 串口帧格式、文件传输与 TCP 流协议 |
| [docs/RELEASE.md](docs/RELEASE.md) | 发布制品与安装说明 |
| [SerialBridge-Delivery-Status.md](SerialBridge-Delivery-Status.md) | 当前版本交付与验证状态 |
| [docs/verification/REVIEW.md](docs/verification/REVIEW.md) | 代码 review 记录 |
| [docs/verification/REVIEW-2026-10-13.md](docs/verification/REVIEW-2026-10-13.md) | 本次全量审查、缺陷修复与本机回归结果 |
| [docs/REFERENCES.md](docs/REFERENCES.md) | 外部参考资料 |

## 许可证

本项目基于 [MIT License](LICENSE) 开源。
