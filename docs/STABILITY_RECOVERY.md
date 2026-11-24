# Stability recovery — native runtime and browser extension

The changes are committed directly to main. Use the matching extension and rebuild the native runtime after pulling the latest main. No ZIP is required.

## Changes
- Mutual HMAC authentication, native/manual pairing, authenticated connection status, and reconnect handling.
- Per-provider target defaults; target changes are blocked during active runs.
- Explicit workspace selector with recent local Zed folders and a manual absolute folder path. Local approval is required for a new binding.
- Saved workspace and session identifiers are scoped by provider, target, and conversation. After bridge restart, the same root is reapproved without requiring a new chat. No automatic selection of a different root.
- Shared workspace locking serializes built-in operations on the same canonical root.
- Steering accepts extra user instructions at the next tool-result or final-response checkpoint. It does not stop the agent or interrupt an in-flight tool.
- Gemini composer fallback, and user-send interception for ChatGPT, DeepSeek and Gemini.
- Uncertain write outcomes are not automatically replayed.

## Local upgrade
Stop the native app and exit its tray icon; update the local checkout from main, rebuild with development/build-native.ps1, reload the unpacked extension at its existing path, and start the rebuilt native runtime. Check that the trusted extension ID matches. Pair via the popup if needed. Select Zed and choose a workspace from the bar. Approve the selected folder in the native app.

## Test status
- Syntax parsing passed for modified JS files.
- 15 isolated WebSocket/crypto-mock assertions on the current background transport passed, including conflicting binding rejection, same-root rebind after reconnect, rejection of invalid server proof and ambiguous write outcomes.
- The Node WebCrypto authentication regression test and Rust workspace binding integration test are included, but not executed here against a native build.
- Native Windows build, live browser/provider DOM, manual approval/restart, multi-window behavior and benchmarks are not verified remotely. Full provider coverage, repository restructuring, durable replay across bridge restart, exact active-window enumeration and resource/performance measurements remain open.
- Do not treat this as a production-certified release. Recent Zed folders are not proof of currently open windows; VS Code discovery lists the most recent local folder, with manual path selection available.
- During a pending tool call, stopping the browser loop does not guarantee cancellation of the native operation. Verify outcome before repeating a write.

## Regression commands
```powershell
Get-ChildItem .\snapgent-extension\test-*.js | ForEach-Object { node $_.FullName; if ($LASTEXITCODE -ne 0) { throw "Test failed: $($_.Name)" } }
cargo test --manifest-path rust/Cargo.toml --workspace
```
