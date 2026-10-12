# SerialBridge Alpha 2 — 交付与验证状态

**版本：** `0.1.0-alpha.2`
**日期：** 2026-10-12
**阶段：** 在实际 Alpha 1 源码基础上继续开发、补测试并准备三平台构建。

## 已执行的本地验证

| 检查 | 实际结果 | 范围 |
|---|---|---|
| Rust Core 单元/集成测试 | 46 通过、0 失败 | 配置、分帧、API、真实 HTTP/WebSocket、RPC、文件传输及异常状态 |
| Rust fmt / Core Clippy | 通过，Clippy 使用 `-D warnings` | 包含 Core 所有测试目标 |
| TypeScript 构建 / Node 测试 | 20 通过、0 失败 | 扩展策略/生命周期与 UI 模型；Chrome API 使用测试替身 |
| 真实双节点和 live UI | 21 项通过 | 两个独立 headless 进程、loopback TCP、真实 HTTP/Core API 和 Chromium UI，无 API 拦截 |
| UI demo/HTTP fixture smoke | 9 项通过 | 明确使用模拟 API，不算真实 Core 联调 |
| 离线 UI DOM | 7 项通过 | 原生命令测试替身，不算原生桌面测试 |
| 扩展 ZIP 检查 | 2 项通过 | ZIP 根目录 manifest、引用产物、可重复字节输出、缺少构建产物时失败 |
| Actions 配置 | actionlint 通过 | 配置检查，不等于远程工作流已经通过 |

本地证据存放于 `docs/verification/alpha2/`。Alpha 1 的旧日志保留为历史记录，不能替代 Alpha 2 的验证。

## 本轮修复

1. 相对配置文件名没有父目录时，初始化与保存使用当前目录，解决 README 开发入口失败。
2. 串口会话独立轮询读、写，保留单写入循环，解决小缓冲区下双向二进制写入互相阻塞。测试使用 1024 字节双工缓冲和双向 8192 字节写入。
3. 取消接收上传立即关闭文件、清理暂存、更新取消状态并释放容量，不再依赖一分钟清理定时器。
4. 创建或发布上传文件发生 I/O 错误时，任务进入失败终态并释放容量，避免永久显示进行中。
5. 补齐 API 配置保存/拒绝、真实 HTTP 限制与超时、WebSocket 配对/锁定/撤销/二进制 ACK、文件分块/校验/清理等回归测试。
6. 保留真实依赖锁文件；Node 测试明确在同一隔离上下文执行，确保当前 Node 环境报告实际 20 项测试而非仅报告四个文件。
7. 独立 review 后补充会话归属：断线只取消该对端会话的未完成任务并释放共享句柄，保留独立本机工作和已完成源缓存。
8. 上传最终落盘期间仍保留可取消/清理的句柄；拉取的对端句柄清理移到可取消分支之外，解决取消后的资源泄漏。

上述产品缺陷均有修复前失败、修复后通过的测试记录。保留已有架构、四页面 UI 和有界单块应答传输；没有添加数据库、续传、滑动窗口、远程 shell 或新权限模型。

## 三平台与真实扩展的验证方式

`.github/workflows/check.yml` 在 Windows、macOS 和 Linux 分别运行 Node/Core 检查与真实双进程流程，然后编译 Tauri、生成安装包并启动实际桌面进程检查嵌入 Core。

Linux 还执行 `tests/e2e.py --extension`，加载实际 Chromium MV3 扩展，验证配对、对端 browser.fetch、浏览器下载经 Core/peer 到达接收端、SHA-256 和撤销配对。**只有该 CI 步骤实际通过，才计为真实扩展验证。**

当前开发云环境禁止安装自定义浏览器扩展，且无法以 root 安装 WebKitGTK/GTK 开发库；本地 Tauri check 因缺少 glib/GTK 系统开发库失败，未声称它已通过。对应平台结果以 [GitHub Actions](https://github.com/kiliter/SerialBridge/actions) 中匹配构建提交的运行记录为准。

四类 Actions 制品：

- `SerialBridge-Windows`：x64 NSIS 安装程序。
- `SerialBridge-macOS`：Apple Silicon + Intel universal DMG。
- `SerialBridge-Linux`：x64 DEB 与 AppImage。
- `SerialBridge-Chrome`：ZIP 根目录包含 manifest.json 的未打包扩展。

只有三个平台作业全部成功，发布作业才会生成对应提交的 Alpha prerelease 和 SHA256SUMS。Linux 的两个格式属于同一平台制品组。安装与限制见 `docs/RELEASE.md`。

## Review 与复现

开发采用独立分支；已完成一次独立代码 review。没有 Critical 问题，发现的三项 Important 生命周期问题均已用失败测试复现并修复，随后完整 Core、Node、真实双节点/UI 测试通过。报告及修复证据见 `docs/verification/REVIEW.md`。

```bash
npm ci
npm test
cargo fmt --all -- --check
cargo clippy -p serialbridge-core --all-targets --locked -- -D warnings
cargo test -p serialbridge-core --locked
cargo build -p serialbridge-core --locked
python tests/e2e.py --browser
python tests/test_packaging.py
```

浏览器检查需要 `requirements-test.txt` 和 Chromium。`--extension` 需要所在机器允许未打包扩展。Windows 排除两个 Unix 文件权限/删除测试，其余 Core 流程同样执行。

## 保留的 Alpha 边界

- 默认来源白名单为空；API Token 与扩展凭据分离，只监听 loopback。
- 一个串口对端、一个活动扩展；CRC32 和 SHA-256 表示完整性，不提供身份认证或加密。
- 默认 115200 / 8N1，单文件 100 MiB、暂存配额 1 GiB；任务与日志在内存，重启不恢复历史。
- 文件传输不提供自动续传、HTTP Range、高吞吐滑动窗口或自动重放 HTTP 操作。
- 不自动删除完成的源缓存；保留 UI/接口显式移除语义。
- 拒绝重定向；浏览器登录态兼容性仍需针对目标网站验证。
- FT232RL 实际双机、热插拔与速率稳定性，原生托盘/安装交互和逐站点登录态没有被软件 fixture 测试代替。
- 安装包未使用发布者签名或 macOS 公证；Chrome ZIP 不是商店发布包或签名 CRX。
