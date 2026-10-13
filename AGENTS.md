# AGENTS.md

面向在本仓库中工作的 AI 编码代理与贡献者。用户文档见 [README.md](README.md)。

## 沟通与文档语言

- 始终使用中文回复。
- 生成或修改的文档使用中文，专业术语（如 Core、RPC、Token、Origin、MV3）可以保留原文。
- 以当前源码、配置和测试为唯一事实依据；`docs/` 中的旧记录与代码冲突时以代码为准，并在回复中指出差异。

## 项目概览

SerialBridge 通过 USB 串口连接两台对等节点，把本机 loopback HTTP 请求转发给对端，由对端用 `reqwest` 或已配对的 Chromium 扩展执行，并支持文件下载、拉取和推送。另有 TCP 转发：A 端监听本地端口，B 端连接 A 指定的目标地址，字节原样透传（典型用法是承载 Xray/Mihomo 的 VLESS）。当前版本 `0.0.1`。

| 目录 | 内容 |
|---|---|
| `crates/core` | Rust 库 `serialbridge_core` 与 CLI `serialbridge`：`protocol`（SBR1 帧）、`transport`（串口/开发用 TCP）、`node`（会话与 RPC）、`api`（Axum HTTP）、`transfer`（文件任务）、`http_executor`、`extension`（扩展 WebSocket）、`tunnel`（TCP 转发与流控）、`config`、`error` |
| `crates/core/tests` | 集成测试，共享工具在 `support/mod.rs` |
| `crates/desktop` | Tauri 2 桌面壳，内嵌 Core，托盘与窗口行为 |
| `ui/src` → `ui/dist` | TypeScript Web 控制台（总览、日志、传输、设置） |
| `browser-extension/src` + `public` → `dist` | Chromium MV3 扩展 |
| `tests/` | Node 测试（`*.test.mjs`）与 Python 端到端/打包/UI 测试 |
| `docs/` | `API.md`、`PROTOCOL.md`、`RELEASE.md`、`releases/`（正式版本发布说明）、设计稿、计划与验证证据 |

## 常用命令

```bash
npm ci
npm run build                 # tsc 编译 ui 与扩展，并复制 public 到 dist；不编译 Rust
npm test                      # build + Node 测试
cargo fmt --all -- --check
cargo clippy -p serialbridge-core --all-targets --locked -- -D warnings
cargo test -p serialbridge-core --locked
cargo check -p serialbridge-desktop --locked   # 需要平台桌面依赖
python tests/e2e.py           # 两个真实 headless 进程（loopback TCP），含 TCP 转发流程
python tests/e2e.py --browser # 需 requirements-test.txt 与 Playwright Chromium
python tests/test_packaging.py
python scripts/check-source.py  # 仅静态检查，不是编译或运行测试
```

工作区默认成员只有 `crates/core`；桌面 crate 需用 `-p serialbridge-desktop` 显式指定。依赖操作使用 `npm ci` 与 `--locked`，不要随意更新 `Cargo.lock` / `package-lock.json`。

## 修改约定

- 修改 Rust 后至少运行 `cargo fmt`、Core Clippy（`-D warnings`）和 `cargo test -p serialbridge-core --locked`。
- 修改 `ui/` 或 `browser-extension/` 的 `src`/`public` 后运行 `npm test`，并一并提交重新生成的 `dist/`（产物入库，打包与测试直接使用 `dist`）。不要手工编辑 `dist/`。
- 修复缺陷先写能复现的失败测试，再修复；保持改动与问题相称，不做投机性抽象。
- 修改 HTTP API 或线协议时同步更新 `docs/API.md` / `docs/PROTOCOL.md` 和 README 中的示例。
- 版本号同时存在于 `Cargo.toml`（workspace）、`package.json`、`crates/desktop/tauri.conf.json` 和 `browser-extension/public/manifest.json`（`version` 为 4 段数字，`version_name` 为语义版本），升级时需保持一致。
- `MANIFEST.sha256` 记录已跟踪文件的 SHA-256；修改文件后需同步更新对应条目，可用 `shasum -a 256 -c MANIFEST.sha256` 校验。
- 提交信息使用 Conventional Commits 前缀（`fix:`、`docs:`、`chore:` 等），英文简述。

## 必须保持的设计约束

- 两端是对等节点，不引入固定 Host/Client 或按操作系统区分的角色。
- HTTP API 只监听 `127.0.0.1`，受保护接口必须校验 Bearer Token；API Token 与扩展凭据分离。
- 默认拒绝：来源白名单为空、方法只允许 GET/HEAD、精确匹配协议+主机+端口、重定向一律拒绝、禁止敏感请求头。
- 扩展不注入页面脚本、不导出 Cookie、不执行任意 JS；扩展离线时不自动回退到普通 HTTP。
- 链路错误不自动重放有副作用的请求。
- 文件传输保持有界单块请求/应答（TCP 转发的每流额度窗口是另一回事，不适用于文件传输）；完成前必须校验长度和 SHA-256，只发布完整文件；移除记录不删除原共享文件。
- 断线或取消不能遗留活动任务、泄漏传输容量或可访问的半成品文件。
- TCP 转发只透传字节，不解析或改写应用协议（VLESS、TLS 等交给 Xray/Mihomo）。目标地址由 A 端配置决定，不要擅自在 B 端加白名单、目标限制等用户未要求的约束。
- 流控：每条流独立额度（`tunnel::WINDOW`），控制帧走 `PeerLink.ctl` 高优先级队列；串口读循环里不得阻塞等待。
- 不添加数据库、断点续传、文件传输滑动窗口、远程 shell、脚本执行、SOCKS/HTTP 代理入口或新的权限模型，除非用户明确要求。
- 不要自行加入用户没有提出的限制；拿不准时先问。

## 验证边界

- 区分本地单元/集成测试、真实 headless 双进程、浏览器/扩展测试、CI 原生启动和实机验收，报告时说明实际执行了哪些。
- `tests/ui_smoke.py`、`tests/ui_offline_dom.py` 和 `?demo=1` 使用测试替身或模拟数据，不能作为真实 Core 联调证据。
- TCP 转发的 Xray/Mihomo 联调可使用 `/Volumes/Extend/runtime` 下的二进制，仅限 loopback，结束后清理进程与临时目录。
- `--dev-tcp-*` 只用于无硬件测试，不能替代 FT232RL 实机串口验收；网站登录兼容性、托盘行为、签名与公证也需对应环境验证。
- 不要把未执行的命令写成已通过的结果。

## 本机环境

- 依赖、缓存和工具统一放在 `/Volumes/Extend/runtime`：`CARGO_HOME=/Volumes/Extend/runtime/cargo-home`、`npm ci --cache /Volumes/Extend/runtime/npm-cache`、`PLAYWRIGHT_BROWSERS_PATH=/Volumes/Extend/runtime/ms-playwright`；Xray、Mihomo 位于 `runtime/xray`、`runtime/mihomo`。
- Rust 工具链使用 `~/.cargo/bin` 下已安装的 stable（rustup）。

## 安全

- 不要在代码、日志、文档或回复中输出真实 API Token、扩展凭据或本机 `config.toml`（已在 `.gitignore` 中忽略）。
- 推送到 `main` 会触发 CI 并自动发布预发布版本（tag 为 `v{版本}-{sha7}`）；推送 `v{版本}` tag 会发布正式版本（tag 必须与 `package.json` 版本一致，发布说明放在 `docs/releases/{版本}.md`）；推送、发布或改动 `.github/workflows/` 前需获得用户明确授权。
