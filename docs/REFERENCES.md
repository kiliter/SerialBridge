# 开发参考（官方资料）

这些资料用于核对 API 与平台行为，不是本工程已经验证成功的证据。设计、业务实现和测试为本次新写代码。

- Tokio serial async stream: https://docs.rs/tokio-serial/latest/tokio_serial/struct.SerialStream.html
- Axum WebSocketUpgrade: https://docs.rs/axum/latest/axum/extract/ws/struct.WebSocketUpgrade.html
- Tauri 调用 Rust: https://v2.tauri.app/develop/calling-rust/
- Tauri 系统托盘: https://v2.tauri.app/learn/system-tray/
- Tauri 构建前置条件: https://v2.tauri.app/start/prerequisites/
- Chrome MV3 WebSocket 生命周期: https://developer.chrome.com/docs/extensions/how-to/web-platform/websockets
- Chrome 扩展跨域请求: https://developer.chrome.com/docs/extensions/develop/concepts/network-requests
- Chrome 来源匹配: https://developer.chrome.com/docs/extensions/develop/concepts/match-patterns

本次在容器中实际可用的工具是 Node.js 22.16.0、TypeScript 5.8.3、Chromium 144.0.7559.96 和 Python。没有 Rust 工具链，外部依赖获取受 DNS/网络限制；浏览器导航到测试 loopback 地址被管理策略阻止。没有修改或绕过这些策略。
