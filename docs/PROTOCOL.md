# SerialBridge wire protocol 2

协议 2 与 0.0.2 及更早版本（协议 1，magic `SBR1`）不兼容，两端必须同时升级。收到对端的有效 `SBR1` 帧时，本机报告 “Peer speaks SerialBridge protocol 1” 并断开重连。

## 帧

所有多字节整数为大端。UUID 直接用 16 个原始字节。

| 偏移 | 字段 | 长度 |
|---:|---|---:|
| 0 | Magic `SBR2` | 4 |
| 4 | Version `2` | 1 |
| 5 | Kind | 1 |
| 6 | Reserved `0` | 2 |
| 8 | src_link：发送方本次会话的链路 ID | 4 |
| 12 | dst_link：接收方链路 ID（握手前为 0） | 4 |
| 16 | seq：序号（含义见下） | 4 |
| 20 | ack：发送方下一个期望接收的序号（累计确认） | 4 |
| 24 | Payload length（JSON + data） | 4 |
| 28 | Request ID / stream ID | 16 |
| 44 | JSON metadata length | 4 |
| 48 | Header CRC32（覆盖偏移 4..48） | 4 |
| 52 | UTF-8 JSON metadata | 可变 |
| | 原始二进制 data | 可变 |
| | Payload CRC32（覆盖 JSON + data） | 4 |

总帧长 `56 + payload_length`，最大 payload 128 KiB。

解码规则：

- 扫描 magic 重同步；header CRC 错误视为噪音，前进 1 字节继续扫描，不终止链路。
- header CRC 正确但版本、Kind 或长度非法，属于协议错误，终止链路（错误长度在等待/分配完整内容前拒绝）。
- header 正确、payload CRC 错误：丢弃该帧，并把可信的 header（src/dst/kind/seq）交给链路层，用于立即请求重传。

## Kind

| Kind | 名称 | 是否编号 |
|---:|---|---|
| 1 | HELLO（握手与保活） | 否 |
| 2 | REQUEST | 是 |
| 3 | RESPONSE | 是 |
| 4 | 保留（协议 1 的 HEARTBEAT，现由 HELLO 承担） | — |
| 5 | CANCEL | 是 |
| 6–11 | TCP 流（见下文） | 是 |
| 12 | LINK_ACK：纯确认 | 否 |
| 13 | LINK_NAK：请求重传 `seq` | 否 |

## 链路层可靠传输

串口线上的位错误不再拆掉会话，而是由链路层重传（selective repeat ARQ）：

- 每个会话随机生成非零 32 位链路 ID。编号帧在每个方向各自从 0 递增（回绕计算），每一帧（包括 HELLO/ACK/NAK）都在 `ack` 中捎带累计确认。
- 接收方按序交付给上层：重复帧只重新确认不重复交付；超前帧暂存（最多领先 512 帧），并为缺口发送 NAK。payload 损坏的编号帧立即 NAK 其序号（每次损坏到达都会 NAK）；同一缺口的重复 NAK 间隔不少于 300 ms。
- 发送窗口：普通数据最多 256 帧 / 256 KiB 未确认；控制帧（流 ACK、RESET、RPC 应答等）额外允许 256 帧 / 64 KiB，避免流控被批量数据卡住；窗口为空时总允许发送一帧。
- 重传：收到 NAK 立即重传对应帧；未确认的队首超时（RTO = 0.3 s + 按波特率估算的本端未确认字节与对端窗口传输时间，指数退避最多 8 倍，上限 30 s）则重传队首。发送方空闲且仍有未确认帧时，约 50 ms（加上按波特率估算的排队时间）后发送一次 LINK_ACK 探测；HELLO/LINK_ACK 的 `seq` 携带发送方下一序号，接收方据此发现丢失的尾帧并 NAK。
- 重传只发生在同一会话内、对端链路 ID 不变的前提下，属于链路层恢复；会话结束后在途 RPC 失败，**不跨会话重放请求**。
- 每 10 秒最多记录一条汇总日志，统计损坏帧、NAK 与重传次数；`/v1/status` 的 `peer.link` 也给出这些累计值。

## 握手、保活与会话变化

HELLO 携带协议版本 `"protocol":2`、进程 instance UUID、节点名、OS、本地扩展在线状态和 `"tunnel":1`。

1. 未握手时每秒发送 HELLO（dst=0 或候选对端 ID）。
2. 收到 HELLO（dst 为 0 或本端 ID）时记录候选对端，并立即回复 dst=候选 ID 的 HELLO。
3. 收到候选对端发来、dst 为本端 ID 的帧后完成握手（两端编号从 0 开始），状态变为在线。

握手后 HELLO 每 5 秒发送一次作为保活；25 秒未收到对端有效帧（含 payload 损坏但 header 正确的帧）则断开。握手后收到另一个链路 ID 的 HELLO（dst=0），说明对端已重启，本端结束会话并重新握手，在途请求失败。收到自己的链路 ID（TX/RX 回环）或相同 instance 时拒绝。

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

单块最大 16 KiB，默认 8 KiB。每个文件仅等待一个块，使用有界请求/响应而不是完整滑动窗口（链路层重传窗口与此无关）。文件传输可以与其他 RPC 交错；只有一个物理串口写入循环，不会出现多个任务交叉写半帧。

## 本机浏览器 WebSocket

地址 `ws://127.0.0.1:<port>/v1/extension/ws`。
只接受 `chrome-extension://<32位 a-p ID>` Origin。连接后 5 秒内必须发送 `{"type":"pair","code":"..."}` 或 `{"type":"auth","token":"..."}`。配对 180 秒/5 次，凭据绑定 Origin。

服务端返回 `ready`，随后 `task.execute` 下发任务，每个任务携带执行策略。JSON 控制消息包括 `task.result`、`task.error`、`task.cancel`、`download.begin`、`download.end`、`chunk.ack`、`ping`、`pong`。

WebSocket 二进制消息：`JSON length:u32BE + JSON metadata + raw bytes`。metadata 最大 4096 字节，data 最大 65536 字节。文件块 metadata 为 `{"type":"chunk","id":"uuid","offset":0}`。

扩展发送一个块，等待 Rust 写入后返回的 `chunk.ack.offset` 与预期下一 offset 完全一致再继续。确认等待上限 15 秒；扩展整体任务上限 5 分钟，执行节点向对端进行较慢串口搬运发生在浏览器下载完成之后。

`download.begin.total_bytes` 可为已知长度或 null；已知时不得接收超过该长度的数据，结束时实际字节数必须同时匹配该长度和 `download.end.total_bytes`。解压后长度未知时使用 null；HEAD 等无响应体的请求使用 0。

WebSocket 本机请求不经过串口；本地 API token 与扩展凭据不发送给另一节点。

## 非目标

该协议没有提供加密、peer 密钥认证、抗主动篡改、跨会话请求重放、断点续传、多节点路由或虚拟网卡。链路层重传只针对同一会话内的线路误码。CRC/SHA 不替代任何安全隔离或保密要求。

## TCP 流

HELLO 带 `"tunnel":1` 表示支持 TCP 转发；对端缺少该字段时，本机拒绝打开流。帧的 ID 字段为流 UUID。

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
- RESET/断链通知独立于数据接收队列，即使 socket 写入阻塞或接收方向已收到 FIN，也会终止该流并释放连接。
- 上限：每节点最多 256 条并发流；对端连接目标超时 10 秒，本机等待 READY 超时 20 秒。
