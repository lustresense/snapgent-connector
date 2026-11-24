# Snapgent Native â€” Migration Map

## Implemented
- [x] Native Windows desktop UI using egui/eframe and OpenGL, without WebView or Electron.
- [x] System tray, bridge lifecycle controls, extension pairing and activity log.
- [x] HMAC authentication and exact origin validation; Python authentication fixture verified.
- [x] Shared Rust workspace engine with 28 tools per editor.
- [x] Custom MCP stdio JSON-RPC transport.
- [x] WebSocket bridge, server routing, HTTP status and SSE endpoints.
- [x] Native local approval workflow, terminal trust disabled by default.
- [x] Release compilation and Windows GUI startup verified.
- [x] Nine automated tests passed, including isolated WebSocket integration.

## Remaining verification
- [ ] Test all 28 operations against legacy behavior fixtures.
- [ ] Test extension pairing and actual agent calls after manual cutover.
- [ ] Test custom MCP interoperability and process termination.
- [ ] Measure RAM and CPU with the bridge active and GUI visible/hidden.
- [ ] Complete security regression testing, including filesystem race conditions.
- [ ] Archive legacy runtime after parity validation.

## Layout
- `app/`: native desktop application and tray.
- `crates/security/`: authentication.
- `crates/workspace/`: Zed and VS Code tools.
- `crates/mcp/`: external stdio MCP transport.
- `crates/bridge/`: WebSocket, HTTP, events, approvals and server routing.
- `app-cases/`: legacy behavior fixtures.
- `research/`: migration reference and architectural notes.
- `../development/build-native.ps1`: reproducible build and test entry point.
- `../dist/native/Snapgent.exe`: release output after successful build.

The migration follows the behavior-first approach documented by rust-anything. Its CLI was not used to generate or certify this port. The original runtime is deliberately retained until the remaining compatibility checks pass.
