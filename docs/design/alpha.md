# SerialBridge Alpha implementation contract

This source is generated anew from the approved conversation and UI; it is not a recovered previous implementation.

* Identical Rust nodes on macOS/Windows, one local loopback HTTP API each. No OS-specific host/client role.
* Tauri 2 embeds the core in one process. A separate headless CLI uses the same core. Do not run both on the same configuration/port.
* Four light-theme pages: overview, logs, transfers, settings. Real API data by default; an explicitly labelled demo mode for offline UI review.
* Browser extension (Chromium MV3, TypeScript) connects to its own node by authenticated loopback WebSocket. Browser credentials stay in the browser; no arbitrary eval or cookie export.
* Serial framed RPC carries JSON metadata and raw binary file blocks. IDs are UUIDs. CRC32 detects corruption; no automatic replay of arbitrary HTTP requests. No encryption or hardware isolation claim.
* Downloads: executing node spools a bounded file, requester pulls verified binary chunks into a .part file, SHA-256 validates completion. File pull and push use the same transfer primitives. Alpha uses one outstanding block per file for backpressure, not a high-throughput sliding window.
* Local HTTP is bearer authenticated, browser pairing is short-lived and bound to the extension Origin. Exact target origin + HTTP method allowlists, deny by default. Redirects are disabled in Alpha.
* Tests that cannot be run (Rust, native installers, physical FT232RL) must be clearly listed as NOT VERIFIED. CI configuration is not execution evidence.

## Alpha limits
One serial peer and one active local browser extension per node. No Range resume, no multi-profile selection, no arbitrary shell/script execution. File size, job concurrency, request size and log count are bounded. No automatic retry of POST. Peer connectivity is distinct from extension connectivity.
