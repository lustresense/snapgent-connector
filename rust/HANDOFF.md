# Native Snapgent 3.9.14

## Launch
Open `dist/native/Snapgent.exe` from the existing project tree. No Python, Node, browser renderer or terminal launcher is needed for built-in MCP adapters. Git and tools invoked by workspace commands remain external executables.

## Migration
1. Stop the legacy bridge before starting the native bridge, since both use ports 17613 and 17614.
2. Reload the hardened unpacked extension from `snapgent-extension/` in chrome://extensions.
3. Enter its extension ID in the desktop Settings page, save it, and copy the pairing credential into the extension's Pair Bridge panel.
4. Start the native bridge. Check the server catalog and test a read-only workspace operation before allowing terminal operations.
5. Use Activity for logs, Open file for saved logs, or Follow in terminal for an optional live PowerShell log viewer.

Closing the desktop window hides it to the system tray if available. Quit from the tray to stop the bridge. Terminal trust is off by default and is scoped to the current bridge session.

## Project layout
- `rust/app/`: native desktop application, theme, settings and tray.
- `rust/crates/bridge/`: transport, routing, events and local approval.
- `rust/crates/mcp/`: external stdio MCP client.
- `rust/crates/workspace/`: built-in Rust workspace adapters.
- `rust/crates/security/`: authentication and origin validation.
- `rust/app-cases/`: captured compatibility fixtures.
- `rust/DESIGN.md`: extension-derived visual direction and Anti-Slop checks.
- `development/build-native.ps1`: format checks, tests and release packaging.
- `dist/native/Snapgent.exe`: release executable.
- `snapgent-extension/`: browser-side JavaScript integration.

The existing root Python runtime and launchers are retained for rollback and the current live connection. Do not remove or relocate them while the old bridge is running.

## Validation
Release built successfully. Nine automated tests passed, including the authentication fixture, lifecycle, wire protocol and workspace checks. The release executable was launched successfully and Windows reported a responsive window. At idle with the bridge stopped, working set was 98.36 MiB, private memory 51.31 MiB, and CPU consumption over a five-second sample was 0.312% of one logical core. Executable size: 7,481,856 bytes.

Still pending: visual inspection of the redesigned window, live extension pairing and full tool parity, external MCP lifecycle coverage, tray/active-bridge benchmarks, and further security regression testing. These measurements do not establish a RAM reduction versus the legacy runtime. The running legacy bridge was not interrupted.
