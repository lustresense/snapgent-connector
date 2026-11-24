# Stability recovery candidate

Base: upstream-compatible browser extension and hardened Rust/Python bridge at fff1497.
This branch restores mutual HMAC authentication before marking the socket ready.
It keeps the exact trusted extension origin policy, credential storage in extension-origin IndexedDB, and existing terminal approvals.

## Install and verify (Windows)

1. Stop all old bridge instances and start the existing native Rust bridge. Do not run Python and Rust together on the same port.
2. Replace the unpacked extension files with this branch's snapgent-extension directory, preserving the original directory path. Reload in chrome://extensions and refresh AI tabs.
3. Check the extension ID matches the trusted ID in the native app. Enable automatic pairing from the app if available. In the extension popup use Pair bridge with an empty field for native pairing, or enter the local credential from the bridge settings. Never put credentials in chat.
4. Check the popup shows Connected and the Zed tool catalogue. Use a read-only command in each provider. Disconnect and reconnect the bridge when idle, then verify the same target.
5. While an agent is running, type in the overlay steering field, click Steer, and verify that the instruction is appended to the next result or final-response continuation without stopping the agent.

## Verification status

- JavaScript syntax parsed for the six changed JS files.
- Eight transport assertions passed in an isolated WebSocket/crypto mock using the actual background.js source from the first patch. This does not prove the live bridge or real cryptography.
- test-auth-transport.js added to exercise the real HMAC proof via Node WebCrypto. Run it and the existing test-*.js suites locally; Node tests have not been executed against this branch in a browser/Windows environment.
- No Windows runtime test, benchmark, or live provider DOM test has been performed in this remote session.
- Steering and target selection changes are candidate features, not live verified. A provider may still need adjustments to its send hooks.
- Explicit multi-workspace selection, persistent workspace binding across restart, durable request replay across sockets, complete provider regression testing, and repository restructuring remain open. Current legacy-compatible RPCs do not provide per-conversation workspace isolation. Do not run simultaneous writes to different projects until explicit binding is implemented and tested.
- The main branch is intentionally unchanged until candidate verification.

## Regression commands

```powershell
Get-ChildItem .\snapgent-extension\test-*.js | ForEach-Object { node $_.FullName; if ($LASTEXITCODE -ne 0) { throw "Test failed: $($_.Name)" } }
```
