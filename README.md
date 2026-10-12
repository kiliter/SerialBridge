# SerialBridge — Source Alpha

**当前版本：`0.1.0-alpha.2`。这是继续验证和修复后的 Alpha。**
请先阅读根目录 `SerialBridge-Delivery-Status.md`。Core 单元/集成测试、真实同机双节点和 live UI 流程已在本地执行。三平台安装包及真实 Chromium 扩展由 GitHub Actions 检查和构建，结果以对应运行记录为准。
下载与平台说明见 [GitHub Releases](https://github.com/kiliter/SerialBridge/releases) 和 [docs/RELEASE.md](docs/RELEASE.md)。FT232RL 双机链路与网站登录态仍需实机验收。

当前已验证发布：[Alpha 2](https://github.com/kiliter/SerialBridge/releases/tag/v0.1.0-alpha.2-8c03980)；[成功构建记录](https://github.com/kiliter/SerialBridge/actions/runs/38285139819)。包含 Windows、macOS universal、Linux 和 Chrome 扩展四个制品组。

## 包含什么

- `crates/core`：Rust 对等节点库与 `serialbridge` 命令行程序。两个节点都提供 loopback HTTP API；没有固定 Mac/Win 或 Host/Client 方向。
- `crates/desktop`：Tauri 2 桌面入口，嵌入同一个 core；系统托盘、窗口关闭后隐藏、真实 API 适配器。
- `ui`：TypeScript + CSS 四页面控制台：总览、日志、传输、设置。复用已批准的浅色视觉语言。
- `browser-extension`：Chromium MV3 扩展。通过本机 WebSocket 配对，执行授权的 `browser.fetch` 与 `browser.download`。
- `tests`、`crates/core/tests`：前端/扩展逻辑测试、UI 测试脚本、Rust 测试源码。
- `docs/verification`：本次真实执行记录，失败/受限记录也保留。
- `docs/design/approved-prototype.html`：先前批准的设计参考；不是实际客户端入口。

```text
本机程序 → 本机 HTTP :9527 → Core A ⇄ USB 串口 ⇄ Core B
                                                     ├─ reqwest HTTP
                                                     └─ WebSocket → 本机浏览器扩展
```

## 1. 构建前的检查

需要 Node.js 22.8+、npm，以及 Rust stable。Windows 桌面构建还需要对应的 MSVC 构建工具与 WebView2；macOS 需要 Xcode Command Line Tools。平台准备步骤以 Tauri 官方文档为准：见 `docs/REFERENCES.md`。

在工程根目录执行：

```bash
npm ci
npm test
cargo test -p serialbridge-core --locked
cargo check -p serialbridge-desktop --locked
```

已保留实际生成的 `Cargo.lock` 和 `package-lock.json`；请使用 `--locked` 和 `npm ci` 保持依赖一致。Linux 桌面开发需要 WebKitGTK 4.1、GTK 3、Ayatana AppIndicator 和相关开发库，安装命令可见 `.github/workflows/check.yml`。Core/headless 不需要桌面开发库。

`npm run build` 只构建 UI 与浏览器扩展，不编译 Rust。包中已包含本次构建出的 `ui/dist` 和 `browser-extension/dist`。

## 2. 桌面启动

```bash
npm run desktop:dev
```

默认配置位于：

- macOS：`~/.serialbridge/config.toml`
- Windows：`%USERPROFILE%\.serialbridge\config.toml`
- 可通过 `SERIALBRIDGE_CONFIG` 环境变量指定其他文件。

首次启动创建本地随机 API Token、下载目录和共享目录。默认来源白名单为空，所有外部 HTTP 请求均被拒绝。打开「设置」选择真实串口并配置允许的目标来源。

桌面程序自己启动 core，不需要再启动 headless 进程。不要同时运行桌面和 headless 占用同一端口/串口。关闭窗口隐藏到托盘；托盘菜单「退出 SerialBridge」终止服务。托盘行为未做 Mac/Win 实机验证。

构建原生应用：

```bash
npm run desktop:build
```

没有提供已签名安装包。签名、公证、安装权限与驱动部署不在本次验证范围。

## 3. Headless 模式

```bash
cargo run -p serialbridge-core -- init
cargo run -p serialbridge-core -- ports
cargo run -p serialbridge-core -- serve
```

读取本机访问 Token（不要发给对端或粘入共享日志）：

```bash
cargo run -p serialbridge-core -- token
```

使用独立配置：

```bash
cargo run -p serialbridge-core -- --config /path/to/node-a.toml serve
```

`configs/node-a.toml` / `configs/node-b.toml` 是示例；串口名字必须替换为实际值，两台机器的波特率必须一致。示例中系统名仅作展示，不限制角色方向。

## 4. 浏览器扩展

1. 构建后，在允许开发者扩展的 Chromium 浏览器中加载 `browser-extension/dist` 为未打包扩展。不要加载 `src`。
2. 在桌面设置点击「生成配对码」。
3. 打开扩展弹窗，填写本机 HTTP 端口和 12 位配对码。配对码有效 180 秒、最多 5 次尝试。
4. 在扩展弹窗对目标网站发起用户授权；同时在执行节点设置中填入精确来源，例如 `https://example.com`。
5. 通过发起节点 HTTP 调用 `browser.fetch`。执行节点必须有已连接扩展；发起节点不要求安装扩展。

扩展的 `fetch` 在浏览器规则允许的情况下使用当前浏览器凭据，但不会保证兼容所有站点登录方式。不注入页面脚本、不导出 Cookie、不执行任意 JS、不自动从浏览器执行器回退至普通 HTTP。

只允许一个已配对扩展 Origin 和一个活动连接。重新配对会替换原连接。解除配对使长期凭据失效。无后台常驻保证；浏览器关闭或扩展被终止时任务会失败。

## 5. HTTP 调用

以下命令在**发起节点**执行；两端 API 一致。`SERIALBRIDGE_TOKEN` 是这台电脑自己的本地 API Token。

```bash
curl http://127.0.0.1:9527/v1/health \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN"

curl -X POST http://127.0.0.1:9527/v1/rpc \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"target":"peer","method":"browser.fetch","params":{"url":"https://example.com/api","method":"GET"}}'
```

普通 HTTP 使用 `http.fetch`，本机执行将 `target` 改为 `local`。`system.ping` 不访问网站。

文件下载：

```bash
curl -X POST http://127.0.0.1:9527/v1/downloads \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"target":"peer","executor":"browser","url":"https://example.com/report.zip","filename":"report.zip"}'
```

接口返回 `202` 和任务 ID。通过 `GET /v1/downloads/{id}` 查询状态，完成后通过 `GET /v1/downloads/{id}/content` 获取文件。`executor` 可为 `http` 或 `browser`。

拉取对端共享目录中的文件：

```bash
curl -X POST http://127.0.0.1:9527/v1/files/pull \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/json' -d '{"filename":"report.zip"}'
```

推送本地文件：

```bash
curl -X PUT http://127.0.0.1:9527/v1/files/push/report.zip \
  -H "Authorization: Bearer $SERIALBRIDGE_TOKEN" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @report.zip
```

上传必须有 `Content-Length`。CLI/浏览器确定文件大小后即可设置；chunked-only HTTP 上传不属于 Alpha 支持范围。文件先在本机暂存，随后进入串口推送队列。

更多接口、限制、错误码见 `docs/API.md`。

## 6. UI 独立预览

无需 Rust，可以启动已构建的 UI：

```bash
python3 -m http.server 4173 --directory ui/dist
```

访问本机 `http://127.0.0.1:4173/`，输入本地 API Token 连接实际 headless 节点。只允许此固定 UI 开发来源，不开启全域 CORS。

`http://127.0.0.1:4173/?demo=1` 是**显式演示模式**，顶部会显示模拟数据提示。服务连接失败不会自动进入演示。双击 `index.html` 不是受支持的 ES Module 启动方式。

## 7. 无硬件测试入口

Rust 测试中使用 Tokio 双工内存流。还提供显式 `--dev-tcp-listen` / `--dev-tcp-connect`，仅允许 loopback，不能绑定外网或替代实际 USB 验收。

```bash
# 两份独立配置，HTTP 端口分别是 9527、9528
cargo run -p serialbridge-core -- --config node-a.toml serve --dev-tcp-listen 127.0.0.1:19727
cargo run -p serialbridge-core -- --config node-b.toml serve --dev-tcp-connect 127.0.0.1:19727
```

这些命令是待执行的测试步骤，不是本次已通过的测试结果。

## 8. Alpha 边界

- Rust 和真实同机 TCP 软件链路已验证；Tauri/Chrome 的平台结果见 Actions，FT232RL 硬件验收仍待执行。
- 文件采用有界、单块请求/应答流控，没有实现高吞吐滑动窗口、自动续传、HTTP Range 或加密串口。
- 默认单文件 100 MiB、下载目录配额 1 GiB；参数可在停止程序后修改，最大单文件上限 10 GiB。不能承诺串口能高速搬运大文件。
- 任务、句柄、日志在内存中；下载完成的文件落盘保留，程序重启不会恢复任务历史。残留 `.part` 需在停机后清理。
- 单节点最多两个主传输任务、两个暂存上传、64 个等待中 peer RPC、8 个 browser 等待任务；扩展自身最多执行 4 个任务。资源忙时返回错误，不无限排队。
- 源文件和本地暂存文件不会随串口传输成功自动删除；通过 UI/接口移除对应已完成记录，或停机后清理目录。移除记录会删除其所属缓存文件，不删除原共享文件。
- 重定向全部拒绝；Header 有明确限制，跨域 Cookie 行为由浏览器决定。
- 没有开机自启、多浏览器配置、自动升级、数据库、脚本执行和远程 shell。
- 链路 CRC 和文件 SHA-256 仅检测完整性，不提供身份认证或保密。USB 物理对端视为受信任节点，必须在获得授权的网络/设备环境中使用。

## 9. 测试证据

先看 `SerialBridge-Delivery-Status.md`。常规命令：

```bash
npm test
cargo test -p serialbridge-core
```

额外验证入口：

```bash
cargo build -p serialbridge-core --locked
python tests/e2e.py                         # 两个真实 headless 进程、HTTP/TCP 与文件流程
python -m pip install -r requirements-test.txt
python -m playwright install chromium
python tests/e2e.py --browser               # 真实 Core + UI
python tests/e2e.py --extension             # 还加载实际 Chromium 扩展，需要允许未打包扩展
python tests/test_packaging.py
npm run extension:package
```

`tests/ui_smoke.py` 和 `tests/ui_offline_dom.py` 仍用于明确标注的 demo/API 测试替身检查，不能当成真实 Core 或硬件证据。`tests/desktop_smoke.py` 检查已编译原生程序的嵌入 Core 启动；Linux 需在 `xvfb-run` 下执行。

已有源码不等于硬件验收。建议后续依次验证：Rust 构建 → 同机双节点协议 → 浏览器扩展 → 两台真实电脑与串口 → 双向并发 → 小文件 → 较大文件 → 热插拔及异常恢复。
