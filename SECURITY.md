# Snapgent local security

## Recommended runtime

Use the native Rust application launched by `start.bat`. Configure the trusted browser extension ID and pair locally. Keep privileged-operation approvals enabled. The desktop window owns the bridge lifecycle. Closing it requests cancellation, stops listeners and sessions, and bounds runtime shutdown; there is no background/headless server mode. Pairing is manual and does not launch a native messaging process. The legacy Python and Electron runtimes are outside the scope of this migration.

## Security controls

- Loopback listeners on ports 17613 and 17614.
- Exact extension-origin checks and mutual HMAC-SHA256 authentication.
- Explicit local workspace binding and isolation between agent sessions.
- Workspace-relative filesystem paths, with parent traversal, device paths, alternate data streams, symlinks and reparse points rejected.
- Local approvals for workspace binding, privileged terminal and Git operations, and administrative MCP changes.
- Optional external MCP processes require explicit enablement. The native desktop stores executable configuration in `%LOCALAPPDATA%/SnapgentSecurity/mcp-config.json`, not the project checkout.
- Raw filesystem tools reject access to Git, Mercurial and SVN metadata and the bridge security directory. Use approved Git operations instead.
- Restricted inherited environment variables for MCP processes.
- Authenticated local dashboard and event endpoints.

## Limitations

This is not an operating-system sandbox. Approved commands, Git hooks and external MCP executables can access resources or networks permitted to the current OS account. Filesystem checks do not eliminate hard-link attacks, short-name aliases, mapped network drives or race conditions. Stop requests cancel queued local calls and signal running local commands; Windows process-tree termination is best-effort, not rollback. Already completed writes, detached child processes and external MCP side effects may survive cancellation. Verify partial changes before repeating commands. UNC paths, including WSL UNC shares, are not supported by local workspace mode.

The obsolete headless and CLI trust flags no longer enable a bridge without a window. Keep the UI's terminal trust option disabled for untrusted tasks. Tool output submitted to a chat is transmitted to the selected AI provider. Do not expose secrets or private files unnecessarily. Use an isolated VM or low-privilege account for untrusted projects.

No dedicated telemetry endpoint was identified in the reviewed native bridge and extension transport. This is not a guarantee about all dependencies, legacy code or future versions.

## Credentials

Keep the bridge credential private. On Windows it is stored in `%LOCALAPPDATA%/SnapgentSecurity/bridge-token`. Pair directly through the extension, never through an AI conversation. The local dashboard uses HTTP Basic authentication with username `snapgent` and the bridge credential, or Bearer authentication for API clients.

## Verification

Run the native workspace tests and extension tests before deploying changes. Source-level checks and unit tests do not establish live compatibility with every AI website.

Local call recovery uses a bounded in-memory receipt journal in the running bridge. A transient socket disconnect does not cancel a dispatched local command; the extension queries its original receipt after authentication. Missing or evicted receipts and a full bridge restart are reported as unknown, never replayed blindly. Stop and closing the bridge still request cancellation. External MCP calls are not covered by this receipt recovery.
