# Alpha 2 independent code review

Reviewed baseline `6aa7dcf` through implementation `d02de72`, including the existing Core, transfer/browser lifecycle, tests and release workflow. A fresh independent reviewer checked the complete current source, not only formatting changes. No Critical findings; three Important findings required fixes before push. No Minor findings were reported.

## Findings and verified repairs

1. **Peer disconnect retained uploads and source-download tasks.** The reviewer reproduced two lingering uploads, BUSY after reconnect, and a source download that continued after its requester disconnected. Peer-created jobs and shared handles now carry their session ownership. Disconnect cancels that session's unfinished work and releases its shared handles. Local uploads/downloads and completed source caches are preserved.
2. **Cancelled finalization removed the only cleanup handle.** The reviewer deterministically dropped file.finish while pending and found an unreapable running job/part file. Finalization now keeps its upload in the map until completion/error, so cancellation and the reaper can still close/delete it. A controlled mutex wait reproduces the cancellation boundary without filesystem timing assumptions.
3. **Cancelled file pull skipped remote handle removal.** The reviewer demonstrated that cancel during serial_transfer consumed one of the peer's 200 artifact handles. Remote handle cleanup now runs outside the cancellable branch, and disconnect also releases session-owned shared handles without deleting shared originals.

`alpha2/review-red.log` records four failing regression cases before these changes. After correction, full Core verification reports **46 passing tests, 0 failed**; Core all-target Clippy with `-D warnings`, Node's 20 tests, the 21 real headless/live UI checks and both ZIP tests also pass. The additional preservation test verifies a completed peer source cache remains accessible after disconnect.

The final fix pass was performed by the implementer and verified using reproducing tests and complete suites. It was not presented as a second independent review. Native platform builds/startup and actual Chromium extension execution are checked by normal CI, because the development environment lacks native development libraries and blocks unpacked extensions.

## Scope decisions

No framework, database, resume protocol, encryption redesign or speculative refactor was added. Tests distinguish actual Core/local HTTP/TCP/WebSocket flows from Chrome API doubles, and do not claim physical serial acceptance or publisher signing/notarization. The four platform artifact groups retain two useful Linux package formats in one group.

## CI follow-up

The first CI run exposed the Node 22 experimental-option spelling, corrected and verified with 20 actual tests on both Node 22 and 24. The next run passed Windows/macOS builds and native startup smoke, but Linux reproduced an I/O race: quota scanning read metadata for a part file concurrently deleted by disconnect cleanup. The local diagnostic confirmed ENOENT; only vanished entries are skipped, retaining all other I/O failures and quota limits. The original regression then passed ten consecutive runs. Post-registration session checks and a stale-session regression cover late resource creation across disconnect. The full local suite now passes **47 Core tests**, Clippy and 21 live process/UI checks. This follow-up was self-checked and tested, not labelled as another independent review.
