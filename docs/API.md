# HTTP API / Alpha

所有受保护接口只监听 `127.0.0.1`，默认端口 9527，要求 `Authorization: Bearer <本机 token>`。`/healthz` 仅公开服务名称；扩展 WebSocket 采用独立的首帧鉴权。

| 方法 | 路径 | 行为 |
|---|---|---|
| GET | /v1/health | 本机、对端、本地扩展、对端扩展状态 |
| GET | /v1/ports | 系统串口枚举 |
| GET | /v1/logs | 最近至多 2000 条事件，按时间正序 |
| GET / PUT | /v1/settings | 脱敏配置 / 保存允许修改的设置 |
| GET | /v1/tunnels | TCP 转发规则的监听状态、活动连接和累计连接数 |
| POST | /v1/serial/reconnect | 关闭现有串口会话后重新连接；在途请求失败 |
| POST / DELETE | /v1/extension/pairing | 生成短期配对码 / 撤销配对 |
| POST | /v1/rpc | system.ping、http.fetch、browser.fetch |
| GET / POST | /v1/downloads | 本机任务列表 / 创建下载 |
| GET | /v1/downloads/{id} | 查询任务详情 |
| DELETE | /v1/downloads/{id} | 活动任务：请求取消；终态任务：移除记录和所属缓存 |
| GET | /v1/downloads/{id}/content | 已完成文件的流式二进制响应 |
| POST | /v1/files/pull | 拉取对端共享根目录中的普通文件 |
| PUT | /v1/files/push/{filename} | 原始 HTTP body 暂存后推送对端，要求 Content-Length |
| GET（升级） | /v1/extension/ws | 扩展 WebSocket；非 HTTP 调用方接口 |

## RPC

```json
{"target":"peer","method":"browser.fetch","params":{"url":"https://example.com/api","method":"GET","headers":{"Accept":"application/json"}},"timeout_ms":60000}
```

`target` 可为 `local` / `peer`。`params.body` 可为字符串或 null，最多 8 KiB。GET/HEAD 不允许非空 body。单条 RPC 超时范围 1–120 秒；文件通过异步任务 API，不受整个文件必须在 120 秒内完成的限制。

```json
{"id":"uuid","result":{"status":200,"headers":{"content-type":"application/json"},"body":"{\"ok\":true}","body_encoding":"utf8"}}
```

非 UTF-8 小型响应使用 `body_base64` 和 `body_encoding: "base64"`。该编码只用于小型 HTTP JSON 返回；文件不使用 Base64 穿过串口。内联响应最多 16 KiB。

远端 HTTP 404/500 是执行结果中的 `status`，不自动改成桥接层失败；下载接口则要求目标 HTTP 返回成功状态。

## 下载

```json
{"target":"peer","executor":"browser","url":"https://example.com/report.zip","filename":"report.zip","headers":{}}
```

`202` 返回完整任务对象，包括 `id`、`status`、`stage`、`received_bytes`、`total_bytes`。阶段 `source_download` 代表执行端网络下载；随后 `serial_transfer` 表示串口传输，此时进度重新从 0 计算。`total_bytes: null` 时不得显示虚假的百分比。

完成条件：全部字节落盘、SHA-256 匹配、临时文件重命名。浏览器下载首先在执行节点暂存。取消是尽力而为，不保证撤回已发生的网络写入。失败任务没有可下载的“成功文件”。

## 设置

可通过 PUT 修改：`node_name`、`serial_port`、`baud_rate`、`allowed_origins`、`allowed_methods`、`auto_reconnect`、`http_port`、`chunk_bytes`、`max_file_bytes`、`max_spool_bytes`、`forwards`。

HTTP 端口改变返回 `restart_required: true`。实际监听端口在 `/v1/health` 中保持真实值直到重启，设置文件保存新值。目录路径与凭据不通过该接口修改。

## TCP 转发

`forwards` 是规则数组，最多 32 条：

```json
{"forwards":[{"listen":"127.0.0.1:10443","target":"127.0.0.1:443"}]}
```

- `listen`：本机监听的 `ip:port`，不可重复。监听非 loopback 地址时，能访问该端口的任何人都能使用隧道，日志会给出警告。
- `target`：对端视角下的 `host:port` 或 `[IPv6]:port`，由对端解析并连接。

保存后监听立即重建，已建立的连接不受影响。客户端连上 `listen` 后，本机通过串口请求对端连接 `target`，然后双向原样透传字节（支持半关闭）。对端连接失败、串口断开或对端节点不支持转发时，本机直接关闭客户端连接。

`GET /v1/tunnels`：

```json
{"forwards":[{"listen":"127.0.0.1:10443","target":"127.0.0.1:443","listening":true,"error":null}],
 "streams":[{"id":"uuid","direction":"outbound","listen":"127.0.0.1:10443","target":"127.0.0.1:443","client":"127.0.0.1:52011","started_at":1760000000000,"bytes_sent":1024,"bytes_received":4096}],
 "total_streams":3}
```

`direction` 为 `outbound`（本机接受、对端连接目标）或 `inbound`（对端发起、本机连接目标）。

## 权限边界

精确来源白名单同时核对协议、域名/IP 和端口，不做后缀匹配。方法默认只允许 GET/HEAD。拒绝 Cookie、Host、Origin、Referer、Content-Length、Connection、Transfer-Encoding、Set-Cookie、Sec-*、Proxy-* 自定义请求头。普通执行器不读取浏览器存储。

浏览器扩展需要：本机配对 + 节点白名单 + 用户授予的网站权限。扩展离线返回错误，不切换执行器。Alpha 一律拒绝重定向，调用方应提供最终 URL。

## 常见桥接错误

`UNAUTHORIZED`、`ORIGIN_NOT_ALLOWED`、`METHOD_NOT_ALLOWED`、`PEER_DISCONNECTED`、`BROWSER_DISCONNECTED`、`BROWSER_ERROR`、`TIMEOUT`、`BUSY`、`FILE_TOO_LARGE`、`SPOOL_QUOTA`、`HASH_MISMATCH`、`RESPONSE_TOO_LARGE`、`INVALID_REQUEST`。

格式：`{"error":{"code":"...","message":"..."}}`。并非所有扩展内部错误都有单独 HTTP 状态；扩展失败会包装为 `BROWSER_ERROR`。

TCP 转发只做字节透传，不解析应用协议、不提供 SOCKS/HTTP 代理入口。无任意命令执行、任意磁盘路径读取或 Cookie 导出接口。
