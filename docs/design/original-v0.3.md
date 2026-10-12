# SerialBridge Spec

**项目名称**: SerialBridge  
**版本**: v0.3  
**目标**: 通过 USB 串口交叉连接，实现任意两台电脑（Win/Mac）之间的 RPC 调用。Host 端通过 Chrome 扩展访问网站。Client 端在本地暴露端口监听请求，本地程序（Rust、CLI、Agent 等）通过本地接口调用，Client 再通过串口与 Host 通信并返回结果。

---

## 1. 背景与目标

用户需要在 Mac 与 Windows 之间进行受控的远程调用：

- Client 发起请求
- Host 端的 Chrome 扩展在真实浏览器环境中执行（携带 Cookie / 登录态）
- 结果通过串口返回给 Client

最终支持四种组合：

- Mac → Win
- Win → Mac
- Win → Win
- Mac → Mac

并额外要求 **Client 在本地暴露端口**，本地程序通过标准接口（HTTP 等）调用，Client 负责串口转发，实现跨语言复用。

---

## 2. 系统架构

```
┌─────────────────────────────┐         串口 (JSON-RPC)         ┌─────────────────────┐
│        Client 机器           │◄──────────────────────────────►│     Host 机器        │
│                             │                                 │                     │
│  ┌───────────────────────┐  │                                 │  • Electron Host    │
│  │ 本地端口 (HTTP 等)     │  │                                 │  • Native Messaging │
│  │ 监听请求              │  │                                 │  • Chrome 扩展      │
│  └──────────▲────────────┘  │                                 │                     │
│             │               │                                 │                     │
│  ┌──────────┴────────────┐  │                                 │                     │
│  │ 本地调用者             │  │                                 │                     │
│  │ • Rust 程序            │  │                                 │                     │
│  │ • CLI                  │  │                                 │                     │
│  │ • Codex / Agent        │  │                                 │                     │
│  │ • 其他语言             │  │                                 │                     │
│  └───────────────────────┘  │                                 │                     │
│                             │                                 │                     │
│  • Electron GUI（可选）     │                                 │                     │
└─────────────────────────────┘                                 └─────────────────────┘
```

### 角色定义

| 角色 | 运行位置 | 职责 |
|------|----------|------|
| **Host** | 有 Chrome 扩展的机器 | 接收串口请求 → 转发给 Chrome 扩展 → 返回结果 |
| **Client** | 另一台机器 | 本地暴露端口监听请求，通过串口转发给 Host，并把结果返回给本地调用者 |

---

## 3. 通信协议

使用 **Newline-Delimited JSON**（一行一个 JSON）。

### 请求格式

```json
{
  "id": "uuid-or-number",
  "method": "fetch",
  "params": {
    "url": "https://example.com/api",
    "method": "GET",
    "headers": {},
    "body": null
  }
}
```

### 响应格式

```json
{
  "id": "uuid-or-number",
  "result": {
    "status": 200,
    "headers": {},
    "body": "..."
  }
}
```

或错误：

```json
{
  "id": "uuid-or-number",
  "error": {
    "code": -1,
    "message": "timeout"
  }
}
```

### 支持的方法（初期）

| method | 说明 | params |
|--------|------|--------|
| `fetch` | 在 Chrome 中发起请求 | `url`, `method`, `headers`, `body` |
| `ping` | 健康检查 | - |
| `eval` | 在页面执行 JS（可选，后期） | `code` |

---

## 4. 功能需求

### 4.1 Host 模式（Electron）

- [ ] 自动列出可用串口
- [ ] 启动后作为 Native Messaging Host
- [ ] 接收串口请求并转发给 Chrome 扩展
- [ ] 接收扩展返回的结果并通过串口回传
- [ ] UI 显示实时日志、连接状态
- [ ] 支持配置默认串口、波特率（默认 115200）

### 4.2 Client 模式

#### 本地端口服务（核心）
- [ ] 在本地启动 HTTP 服务器（默认 `127.0.0.1:端口`）
- [ ] 接收本地程序发来的请求（JSON）
- [ ] 将请求通过串口转发给 Host
- [ ] 等待 Host 响应后，把结果返回给本地调用者
- [ ] 支持超时、并发请求匹配（通过 `id`）

**本地接口示例**：

```http
POST http://127.0.0.1:9527/rpc
Content-Type: application/json

{
  "method": "fetch",
  "params": {
    "url": "https://example.com/api",
    "method": "GET"
  }
}
```

返回：

```json
{
  "id": "...",
  "result": {
    "status": 200,
    "body": "..."
  }
}
```

#### CLI
- [ ] 提供 CLI 工具，内部调用本地端口
- [ ] 或 CLI 直接作为轻量客户端

```bash
serialbridge fetch --url "https://example.com/api"
# 默认请求本地 127.0.0.1:9527
```

#### 其他调用方式
- Rust / Python / Node / Agent 等均可直接请求本地 HTTP 接口
- 无需各自实现串口逻辑

#### GUI（可选）
- [ ] 显示连接状态、日志
- [ ] 手动测试请求
- [ ] 配置串口和本地端口

### 4.3 Chrome 扩展

- [ ] 通过 Native Messaging 与 Host 通信
- [ ] 执行 `fetch`（`credentials: 'include'`）
- [ ] 支持自定义 headers / body
- [ ] 返回 status + body + headers

### 4.4 Agent / Skill 集成

本地程序直接请求 Client 暴露的端口即可：

```bash
curl -X POST http://127.0.0.1:9527/rpc \
  -H "Content-Type: application/json" \
  -d '{"method":"fetch","params":{"url":"https://example.com/api"}}'
```

或使用 CLI：

```bash
serialbridge fetch --url "https://example.com/api" --json
```

---

## 5. 技术选型

| 层级 | 技术 |
|------|------|
| 桌面框架 | Electron（Host + 可选 GUI） |
| Client 本地服务 | HTTP 服务器（可用 Node 或 Rust 实现） |
| 语言 | TypeScript（Electron Host） + 任意（Client 本地调用者） |
| 串口 | Node `serialport` 或 Rust `serialport` |
| 本地接口 | HTTP JSON-RPC（默认） |
| CLI | 调用本地端口的轻量工具 |
| Chrome 扩展 | Manifest V3 + Native Messaging |
| 协议（串口） | Newline-Delimited JSON |

---

## 6. 项目结构（建议）

```
serialbridge/
├── electron/                 # Host（+ 可选 Client GUI）
│   ├── main/
│   ├── renderer/
│   └── preload/
├── extension/                # Chrome 扩展
│   ├── manifest.json
│   └── background.js
├── client-server/            # Client 本地端口服务（推荐 Rust 或 Node）
│   └── ...
├── cli/                      # 轻量 CLI（调用本地端口）
└── README.md
```

---

## 7. Native Messaging 配置

### Windows
注册表项指向 Host 可执行文件。

### macOS
`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.serialbridge.host.json`

Host manifest 示例：

```json
{
  "name": "com.serialbridge.host",
  "description": "SerialBridge Native Host",
  "path": "/path/to/serialbridge",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://你的扩展ID/"
  ]
}
```

---

## 8. CLI 设计细节（给 Agent 用）

```bash
serialbridge [command] [options]

Commands:
  fetch     发起 fetch 请求
  call      通用 RPC 调用
  ping      测试连接
  ports     列出可用串口

Options:
  --port <port>     串口路径（必填）
  --baud <number>   波特率，默认 115200
  --timeout <ms>    超时时间，默认 15000
  --json            只输出 JSON（Agent 友好）
  --pretty          美化输出
```

**Agent Skill 调用示例**：

```json
{
  "command": "serialbridge fetch --url \"https://api.example.com/data\" --port COM5 --json",
  "parse": "json"
}
```

---

## 9. 非功能需求

- 串口断开自动重连（可选）
- 请求超时可控
- 日志可导出
- Windows / macOS 双平台构建
- CLI 模式无 GUI 启动（`--cli` 或检测无显示器）

---

## 10. 里程碑

| 阶段 | 内容 | 优先级 |
|------|------|--------|
| M1 | 串口双向通信 + JSON 协议 | P0 |
| M2 | Host + Chrome 扩展 Native Messaging | P0 |
| M3 | Client 本地端口服务（HTTP） | P0 |
| M4 | CLI（调用本地端口） | P1 |
| M5 | Electron GUI | P1 |
| M6 | Agent Skill 示例 + 文档 | P1 |
| M7 | 错误处理、重连、多方法 | P2 |

---

## 11. 风险与注意事项

- Native Messaging 需要正确注册 Host，路径和扩展 ID 必须匹配
- 串口权限（macOS 可能需要授权）
- Chrome 扩展必须在 Host 机器上安装并启用
- 高频率请求时注意串口缓冲区

---

## 12. 后续可扩展

- 支持 WebSocket 替代串口（同一局域网）
- 支持文件传输
- 支持多个 Chrome 配置文件
- 加密通信（可选）

---

**文档状态**: Draft  
**下一步**: 确认协议与 CLI 接口后开始实现 M1 + M2 + M3。
