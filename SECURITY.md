# Snapgent local security

## Recommended runtime

Use the native Rust application launched by `start.bat`. Configure the trusted browser extension ID and pair locally. Keep privileged-operation approvals enabled. The desktop window owns the bridge lifecycle. Closing it stops the listeners and sessions; there is no background/headless server mode. Pairing is manual and does not launch a native messaging process. The legacy Python and Electron runtimes are outside the scope of this migration.

## Security controls

- Loopback listeners on ports 17613 and 17614.
- Exact extension-origin checks and mutual HMAC-SHA256 authentication.
- Explicit local workspace binding and isolation between agent sessions.
- Workspace-relative filesystem paths, with parent traversal, device paths, alternate data streams, symlinks and reparse points rejected.
- Local approvals for workspace binding, privileged terminal and Git operations, and administrative MCP changes.
- Optional external MCP processes require explicit enablement.
- Restricted inherited environment variables for MCP processes.
- Authenticated local dashboard and event endpoints.

## Limitations

This is not an operating-system sandbox. Approved commands, Git hooks and external MCP executables can access resources or networks permitted to the current OS account. Filesystem checks do not eliminate hard-link attacks, mapped network drives or race conditions. UNC paths, including WSL UNC shares, are not supported by local workspace mode.

The obsolete headless and CLI trust flags no longer enable a bridge without a window. Keep the UI's terminal trust option disabled for untrusted tasks. Tool output submitted to a chat is transmitted to the selected AI provider. Do not expose secrets or private files unnecessarily. Use an isolated VM or low-privilege account for untrusted projects.

No dedicated telemetry endpoint was identified in the reviewed native bridge and extension transport. This is not a guarantee about all dependencies, legacy code or future versions.

## Credentials

Keep the bridge credential private. On Windows it is stored in `%LOCALAPPDATA%/SnapgentSecurity/bridge-token`. Pair directly through the extension, never through an AI conversation. The local dashboard uses HTTP Basic authentication with username `snapgent` and the bridge credential, or Bearer authentication for API clients.

## Verification

Run the native workspace tests and extension tests before deploying changes. Source-level checks and unit tests do not establish live compatibility with every AI website.
