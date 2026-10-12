# SerialBridge Source Alpha — 交付与验证状态

**版本：** `0.1.0-alpha.1`  
**日期：** 2026-10-12  
**交付类型：** 新生成的开发版源码工程；不是原生安装包，也不是已完成实机验收的发布版。

## 1. 本次交付结论

本次实际创建了 Rust Core、Tauri 桌面入口、TypeScript 四页面 UI、浏览器扩展、配置示例、测试和文档。此前回复中声称存在的源码包没有实际生成；本次是重新实现，不是恢复先前不存在的文件，也不沿用此前未经证实的测试声明。

**已验证：TypeScript 构建、20 项 Node 逻辑测试、7 项带测试替身的离线 UI DOM 检查。**  
**未验证：Rust 编译/测试、Tauri Windows/macOS 构建、真实浏览器扩展联调、FT232RL 双机链路。**

源码可能仍有编译错误、运行时问题或平台差异。不能将“源码已写入压缩包”理解为“完整应用已编译并可直接投入使用”。应先在具备 Rust 工具链的机器上编译和修复，再继续实机联调。

### 两个交付文件

- `SerialBridge-Source-Alpha.zip`：根目录为 `serialbridge-alpha/`，包含实际源码和本次构建出的 UI/扩展静态产物。
- `SerialBridge-Delivery-Status.md`：本报告。压缩包根目录内也有内容相同的副本。

压缩包内提供 `MANIFEST.sha256`，用于核对各文件字节内容；它不是发布者签名或软件质量保证。压缩包不包含 `node_modules`、Rust 依赖缓存、`target`、Windows/macOS 安装程序或字体文件。

## 2. 源码内容与验证范围

| 模块 | 源码中包含的内容 | 实际验证状态 |
|---|---|---|
| Rust 对等节点 | 相同 Core 可用于两端；串口或显式开发传输；本地 HTTP；并发请求匹配；断线状态 | 源码和测试已写入；没有编译或运行验证 |
| 串口分帧 | 帧头、版本、UUID、JSON 控制数据、原始二进制、CRC32、大小限制 | 有 Rust 测试源码；未执行，未接硬件 |
| HTTP 执行器 | 精确来源白名单、方法/Header 限制、有界响应和文件下载 | Rust 实现未执行 |
| 浏览器扩展 | MV3、配对、WebSocket、心跳、授权检查、fetch、文件分块和确认 | TypeScript 构建通过；相关逻辑在 Node 测试中通过；未在真实扩展环境联调 |
| 文件传输 | 源节点先落盘；分块拉取/推送；任务进度、取消、SHA-256；接收完成后发布文件 | 核心 Rust 链路未执行；扩展分块协议仅在测试替身环境验证 |
| 桌面外壳 | Tauri 2 嵌入 Core；系统托盘；关闭窗口隐藏；本地命令适配器 | 源码和配置存在；原生构建、托盘行为未验证 |
| 四页面 UI | 总览、日志、文件传输、设置；本地/对端扩展分别展示；配对与基本配置 | TypeScript 构建通过；7 项离线 DOM 交互检查通过；不是实际 Core 联调 |
| 发布工作流 | Windows/macOS 检查工作流和构建入口 | 仅提供配置；没有执行 CI，没有创建或推送远程仓库 |

## 3. 本次执行的检查

### 3.1 TypeScript 构建与 Node 测试：通过

执行命令：

```bash
npm test
```

结果：**20 tests / 20 pass / 0 fail / 0 skipped**。`npm test` 会先构建 UI 和扩展。

覆盖内容包括：精确来源、端口和协议校验；危险 Header 与方法拒绝；文件名安全；无 Content-Length 时的响应体大小上限；权限检查期间取消后不发出请求；Cookie 请求选项与重定向拒绝；文件块大小和 ACK 的传输标识/偏移校验；ACK 超时、取消、连接重置；UI 对端离线状态、日志过滤、未知进度和 HTML 转义。

其中扩展生命周期测试加载实际编译后的 `background.js`，但用测试替身提供 Chrome API、WebSocket 和 fetch。它验证了配对、任务执行、拒绝不允许的请求、140000 字节三块传输和断线行为。**这不是浏览器扩展实机测试。**

最新证据：`docs/verification/10-delivery-node-tests.log`。较早的 RED/GREEN 记录也保留于同目录。

### 3.2 离线 UI DOM 检查：通过，但使用模拟 API

执行命令：

```bash
node tests/build-offline-ui.mjs
python tests/ui_offline_dom.py
```

结果：**7 项通过**。

1. 四类连接状态卡片正确展示，测试字符串按文本转义。
2. 日志筛选、详情与暂停交互工作。
3. 导出按钮调用预期的原生命令测试替身。
4. 创建下载和取消更新任务卡片。
5. 设置保存与配对控件调用预期接口。
6. 对端断开时，对端浏览器状态转为未知而非继续显示在线。
7. Core 获取失败时清除在线状态，不自动进入乐观演示模式。

测试在 Chromium 的 `about:blank` 上渲染本地 HTML/JS，注入明确标记的原生命令测试替身，不连接 Rust、真实扩展或串口。截图中带有“离线 DOM 测试 / 模拟 API / 未连接硬件”说明。

最新证据：

- `docs/verification/11-delivery-offline-build.log`
- `docs/verification/12-delivery-offline-dom.log`
- `docs/verification/ui-offline-results.json`
- `docs/verification/ui-offline-overview.png`
- `docs/verification/ui-offline-logs.png`
- `docs/verification/ui-offline-transfers.png`
- `docs/verification/ui-offline-settings.png`

### 3.3 正常浏览器导航测试：受限，未通过

运行常规 UI smoke 测试时，浏览器访问 `http://127.0.0.1:4173/?demo=1` 返回 `ERR_BLOCKED_BY_ADMINISTRATOR`。保留此失败记录，未尝试绕过环境策略。

因此，不能据离线 DOM 测试声称真实 loopback UI 或浏览器扩展端到端已通过。

证据：`docs/verification/05-ui-smoke.log`、`docs/verification/ui-smoke-results.json`。

### 3.4 Rust / Tauri：未验证

尝试 `cargo test -p serialbridge-core` 时，环境返回 `cargo: command not found`（退出码 127）。`rustc` 也不存在。当前源码包含 **16 个 Rust 测试函数**，但它们一个也未在本环境执行。

包括配置、分帧、HTTP 校验、扩展配对、传输和集成测试源码。不能将“存在测试函数”计为“测试通过”。Tauri 同样没有编译、打包或启动。

证据：`docs/verification/rust-attempt.log`、`docs/verification/14-delivery-environment.log`。

### 3.5 静态完整性检查：通过，非编译验证

执行 `python scripts/check-source.py`：JSON/TOML 可以解析、预期源文件/静态产物存在、未发现 `todo!()` / `unimplemented!()` 宏。此检查**不检查 Rust 类型、借用规则、链接或运行时正确性**。

最新证据：`docs/verification/13-delivery-source-sanity.log`。

## 4. 依赖、环境与可复现边界

本次使用 Node.js 22.16.0、TypeScript 5.8.3、Python 3.13 和本机可用 Chromium。准确版本见 `14-delivery-environment.log`。

`npm run build` 使用现有 TypeScript 编译器，未在本环境通过 `npm install` 获取完整桌面工具链。网络依赖下载不可用；未解析 Rust 依赖，未生成或伪造 `Cargo.lock` / npm lockfile。`Cargo.toml` / `package.json` 约束依赖版本，但首次解析后的精确依赖集仍需在目标环境固定。

这意味着本包不是完整离线依赖包，也不是已经证明可重复编译的锁定版本。运行 Tauri 构建需要正常安装工具链和依赖。

本轮有内联自查记录，见 `docs/verification/REVIEW.md`；没有独立审查代理复核。

## 5. 当前 Alpha 功能边界

- 两端代码对等，不固定 Windows/macOS 方向；每节点仅一个物理对端、一个活动浏览器扩展。
- 默认监听本机 `127.0.0.1:9527`。API Token 与扩展凭据分开；目标来源白名单默认空。
- 串口默认 115200、8N1。更高波特率必须进行 FT232RL 及驱动实测，软件测试不能替代。
- 文件是有界分块/单块应答模式，尚未实现高吞吐滑动窗口、HTTP Range、自动断点续传或多流优先级调度。
- 默认单文件 100 MiB、下载目录配额 1 GiB。任务与日志保存在内存；重启不恢复任务索引，完成文件留在磁盘。
- 中断时可能留下 `.part`；串口传输成功不自动删除源节点缓存和发起节点暂存文件，需通过 UI/接口或停机后清理。
- 所有重定向在 Alpha 拒绝；没有任意页面脚本、Cookie 导出、自动浏览器认证适配或远程 shell。
- 浏览器请求是否能复用某个站点的登录态需要逐站点验证，不保证所有网站通用。
- 没有实现开机自启、多浏览器配置、自动升级、持久化任务数据库、原生安装签名和公证。
- USB 物理对端按受信任节点处理；没有串口链路加密/身份认证。CRC32 和 SHA-256 只用于完整性，不等于安全隔离。只能在获准的数据交换环境内使用。

## 6. 目标机器上的下一步验收

下面是**待执行步骤，不是本次已通过的结果**。从解压根目录开始：

```bash
npm install
npm test
cargo test -p serialbridge-core
cargo check -p serialbridge-desktop
npm run desktop:dev
```

先修复编译和单元测试问题，再按 README 的开发传输入口测试两个独立节点。随后验证真实扩展配对和目标站点，再连接两台电脑做 FT232RL 双向并发、小文件、大文件校验、取消、热插拔和掉线恢复。

主要入口：`README.md`；接口约定：`docs/API.md`；串口格式：`docs/PROTOCOL.md`；方案：`docs/design/alpha.md`。`browser-extension/dist` 为本次实际构建产物，需在允许开发者扩展的浏览器中加载；不包含应用商店发布包。

**最终判定：源码 Alpha 可供审查与继续编译联调；尚未达到“解压/安装即可稳定使用”的验收状态。**
