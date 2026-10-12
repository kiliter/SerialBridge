# SerialBridge Alpha verification and release plan

> **For agentic workers:** Execute inline using superpowers:executing-plans. Use one independent whole-branch review before publishing.

**Goal:** Make the delivered Alpha buildable, test its real software flows, fix demonstrated defects, and publish Windows, macOS, Linux and Chrome extension artifacts.

**Architecture:** Keep the existing shared Rust Core, Tauri shell, TypeScript console and Chromium MV3 executor. Keep stop-and-wait binary transfers; add no resume, database, encryption, plugin framework or new UI pages.

**Tech Stack:** Rust stable, Tokio/Axum/reqwest/Tauri 2; TypeScript 5.8.3; Node 22+; GitHub Actions.

**Spec:** `docs/design/alpha.md`, `docs/API.md`, `docs/PROTOCOL.md`, and the user's approved continuation/release instruction.

## Global Constraints

- Nodes remain peers, with loopback-only APIs and explicit browser pairing.
- Preserve default-deny exact origins, no exported browser credentials and no automatic replay of side-effecting requests.
- Test actual production functions and local servers; identify unavoidable Chrome API doubles.
- Tests do not establish physical FT232RL reliability, website login compatibility, installation signing or macOS notarization.
- Keep fixes proportional: address observable errors and lifecycle failures without speculative abstractions.
- User explicitly authorized development, repository push and Actions artifact publication.

## Review Focus

- Cancellation/disconnection must not leave active jobs, leaked transfer capacity or accessible partial success files.
- Two nodes sending simultaneously must complete without transport write/read deadlocks.
- File completion must verify length/hash, publish only complete files and preserve original shared files on removal.
- Browser re-pair/revoke/reconnect must reject obsolete credentials and update actual connection state.
- Packaging must build the same locked source tested by CI and produce all four requested artifact groups.

### Task 1: Reproducible baseline

**Files:** `Cargo.lock`, `package-lock.json`, Rust manifests only when compiler evidence requires a correction.

- [x] Install a minimal stable Rust toolchain and npm development dependencies.
- [x] Run `npm test` and `cargo test -p serialbridge-core -j 4`; preserve initial errors.
- [x] Fix compiler defects minimally, then rerun the affected suite.
- [x] Retain lockfiles and record versions and exact baseline commands.

### Task 2: Core lifecycle and transfer regression coverage

**Files:** `crates/core/src/{config,protocol,node,transport,transfer,api,extension}.rs`, `crates/core/tests/`.

**Interfaces:** Consume existing `Node::call`, `transport::session`, upload/read/finish/remove methods and `api::router`; retain current public API shapes.

- [x] Add tests for frame boundaries, auth/settings, HTTP policies, RPC timeout/disconnect and concurrent calls.
- [x] Add binary transfer tests for empty/multiple chunks, push/pull, checksum failure, cancel and quota/concurrency release.
- [x] For each discovered defect, run a reproducing test before fixing, then run all Core tests.
- [x] Format touched Rust and run compiler/lint checks; commit verified fixes and tests.

### Task 3: Software end-to-end flows

**Files:** integration tests and test utilities; `tests/`, UI/extension source only for reproduced defects.

**Interfaces:** Two independent configs/headless nodes, loopback HTTP/TCP, browser WebSocket messages from `docs/PROTOCOL.md`.

- [x] Start two real headless processes, exercise authenticated bidirectional RPC, HTTP download, push/pull and cancellation against local fixture content.
- [x] Exercise real Core WebSocket pairing/authentication, browser fetch, binary download and revoke/disconnect.
- [x] Run existing Node and DOM suites; add a browser smoke test against actual Core and extension where supported by the environment.
- [x] Preserve commands and distinguish integration coverage from hardware/site-login validation.

### Task 4: Desktop and release workflow

**Files:** `.github/workflows/`, `crates/desktop/`, `scripts/`, `package.json` and release documentation.

- [x] Install required Linux native development libraries and run `cargo check -p serialbridge-desktop`.
- [x] Build Chrome ZIP from actual generated extension assets with manifest at its root.
- [x] Configure locked CI tests before packaging; build Windows NSIS, macOS universal DMG and Linux DEB/AppImage as platform artifact groups.
- [x] Configure four named Actions artifacts and a prerelease containing installers and extension ZIP.
- [x] Validate workflow syntax and locally build/check feasible packages.

### Task 5: Review and final verification

**Files:** review evidence, delivery status and README.

- [x] Obtain an independent review of the complete source and diff; resolve critical/important findings with reproducing tests.
- [x] Run complete Rust/Node/integration/browser checks and desktop build; retain actual logs.
- [x] Update current delivery status, review record and build/installation instructions; regenerate manifest if retained.
- [x] Commit the verified source and inspect tracked contents for secrets or machine paths.

### Task 6: Publish and inspect four artifact groups

**Files:** GitHub repository and Actions runs, no unrelated repositories.

- [x] Push reviewed source to `kiliter/SerialBridge`; no force pushes or unrelated remote changes.
- [x] Run CI/package workflow; inspect failures and fix until requested automated jobs pass.
- [x] Verify all four artifact groups exist and release links point to the matching build.
- [x] Report actual test counts, commit/run/release links and remaining physical/platform limitations.
