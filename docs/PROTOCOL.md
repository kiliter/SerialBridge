# SerialBridge wire protocol 1 / Alpha

## 帧

所有多字节整数为大端。UUID 直接用 16 个原始字节。

| 字段 | 长度 |
|---|---:|
| Magic `SBR1` | 4 |
| Version `1` | 1 |
| Kind | 1 |
| Reserved `0` | 2 |
| Payload length（JSON + data） | 4 |
| Request ID / session ID | 16 |
| JSON metadata length | 4 |
| UTF-8 JSON metadata | 可变 |
| 原始二进制 data | 可变 |
| CRC32 | 4 |

总帧长 `36 + payload_length`。CRC32 计算范围为 version 字节至 data 末尾，包含所有头字段（除 magic）和 JSON。最大 payload 128 KiB。错误长度在等待/分配其完整内容前拒绝。magic 前的噪音可以扫描重同步；CRC/版本错误终止当前链路并重新握手，**不重放请求**。

Kind：1 HELLO、2 REQUEST、3 RESPONSE、4 HEARTBEAT、5 CANCEL、6–11 TCP 流（见下文）。

HELLO/HEARTBEAT 使用本次链路 session UUID，携带协议版本、进程 instance UUID、节点名、OS、本地扩展在线状态。正常心跳 5 秒，25 秒未收到对端帧会断开。远端 session 改变会令在途请求失败。

REQUEST 的 metadata：`{"method":"http.fetch","params":{...}}`。
RESPONSE 的 metadata：`{"result":{...}}` 或 `{"error":{"code":"...","message":"..."}}`。
独立 UUID 允许两端同时请求，不依赖奇偶编号或操作系统角色。

## 文件

`file.open`：仅从已配置共享根目录打开安全文件名，创建随机文件句柄并计算 SHA-256。
`file.read`：metadata 指定 id、offset、length，RESPONSE 原始 data 字段带块内容。
`file.create`：提供 filename、total_bytes、可选 SHA-256，创建暂存上传句柄。
`file.write`：metadata 指定 id、offset，REQUEST 原始 data 字段带块内容；返回下一 offset 作为确认。
`file.finish`：检查总大小及预期 SHA-256，完整后发布文件。
`file.remove`：释放句柄；缓存文件可删除，共享原文件不删除。

单块最大 16 KiB，默认 8 KiB。Alpha 每个文件仅等待一个块，使用有界请求/响应而不是完整滑动窗口。文件传输可以与其他 RPC 交错；只有一个物理串口写入循环，不会出现多个任务交叉写半帧。

## 本机浏览器 WebSocket

地址 `ws://127.0.0.1:<port>/v1/extension/ws`。
只接受 `chrome-extension://<32位 a-p ID>` Origin。连接后 5 秒内必须发送 `{"type":"pair","code":"..."}` 或 `{"type":"auth","token":"..."}`。配对 180 秒/5 次，凭据绑定 Origin。

服务端返回 `ready`，随后 `task.execute` 下发任务，每个任务携带执行策略。JSON 控制消息包括 `task.result`、`task.error`、`task.cancel`、`download.begin`、`download.end`、`chunk.ack`、`ping`、`pong`。

WebSocket 二进制消息：`JSON length:u32BE + JSON metadata + raw bytes`。metadata 最大 4096 字节，data 最大 65536 字节。文件块 metadata 为 `{"type":"chunk","id":"uuid","offset":0}`。

扩展发送一个块，等待 Rust 写入后返回的 `chunk.ack.offset` 与预期下一 offset 完全一致再继续。确认等待上限 15 秒；扩展整体任务上限 5 分钟，执行节点向对端进行较慢串口搬运发生在浏览器下载完成之后。

WebSocket 本机请求不经过串口；本地 API token 与扩展凭据不发送给另一节点。

## 非目标

该协议没有提供加密、peer 密钥认证、抗主动篡改、自动重传、断点续传、多节点路由或虚拟网卡。CRC/SHA 不替代任何安全隔离或保密要求。

## TCP 流

HELLO/HEARTBEAT 带 `"tunnel":1` 表示支持 TCP 转发；对端缺少该字段时，本机拒绝打开流。帧的 ID 字段为流 UUID。

| Kind | 名称 | 内容 |
|---:|---|---|
| 6 | STREAM_OPEN | metadata `{"target":"host:port"}`，请求对端连接目标 |
| 7 | STREAM_READY | 对端已连上目标 |
| 8 | STREAM_DATA | 原始 data 字段为 TCP 字节，单帧最多 16 KiB |
| 9 | STREAM_ACK | metadata `{"bytes":n}`，接收方写入本地 socket 后归还额度 |
| 10 | STREAM_FIN | 发送方向已结束（半关闭） |
| 11 | STREAM_RESET | metadata `{"error":{...}}`，立即终止该流 |

- 流控：每条流每个方向 256 KiB 额度，发送前扣减、收到 ACK 后恢复。超出额度的流会被单独重置，不影响链路和其他流。
- 优先级：OPEN/READY/ACK/RESET 走高优先级队列，先于 DATA/FIN 写出，避免大流量连接饿死流控。DATA 与 FIN 同队列，保证 FIN 在最后一块数据之后。
- 结束：两个方向都收到/发出 FIN 后流正常关闭；某条流的本地 socket 出错只重置该流；串口断开或会话变化时，该会话的所有流立即重置，不重放数据。RESET 不会因控制队列暂满而丢弃。
- 上限：每节点最多 256 条并发流；对端连接目标超时 10 秒，本机等待 READY 超时 20 秒。
