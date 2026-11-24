# Snapgent Local Security

## Windows hardened runtime

Use `start.bat`, which delegates to `start-secure.bat` and runs the reviewed Python source in a dedicated virtual environment. The legacy bundled executables are retained but are not used by the new launcher.

## First-time migration

1. Stop the old bridge manually. Do not terminate unrelated processes occupying its ports.
2. Open chrome://extensions and enable Developer mode. Reload the Snapgent extension and copy its extension ID.
3. Run start.bat and enter the trusted extension ID when prompted.
4. Choose No at the unrestricted terminal trust prompt for the safer default.
5. Open `%LOCALAPPDATA%\SnapgentSecurity\bridge-token` locally. Copy the credential directly into Pair Bridge in the extension popup. Never paste it into an AI conversation.
6. Reload the supported AI chat page and check the bridge status. Start or resume an agent session.

The local progress dashboard is at http://127.0.0.1:17614/ and requires HTTP Basic authentication (username snapgent, password the local bridge credential). API clients may use Bearer authentication instead.

## Security controls

- Loopback-only network listeners.
- Exact trusted browser extension origin and mutual HMAC-SHA256 challenge-response before WebSocket RPCs are accepted.
- Private extension-origin credential storage, separate from content-script settings.
- Authenticated HTTP status and SSE endpoints; no wildcard CORS.
- Active agent tab ownership, with cleanup on session release or tab closure.
- Local approval for administrative MCP changes and privileged editor terminal/Git operations by default.
- Workspace pinning across automatic MCP restarts; intentional approved restart permits workspace changes.
- Resolved-path containment, recursive traversal restrictions and Windows device/alternate stream checks.
- Reduced sensitive logging and restricted inherited environment variables. Custom MCP environment settings remain supported; explicit inherit_env lists may be configured locally.
- Dedicated Python virtual environment and pinned dependency versions.

## Limitations and operating advice

These controls are defense in depth, not an operating-system sandbox. An approved shell command, trusted terminal session, custom executable or malicious process running under the same Windows account can access resources permitted to that account. Path checks cannot eliminate every filesystem race or hard-link attack. Treat workspace content and AI-generated commands as untrusted. Review local approval dialogs carefully. Use a separate low-privilege account or isolated VM for untrusted projects.

Tool results returned to an AI conversation are transmitted to that AI provider. Avoid reading credentials or private files into conversations. Historical logs from the older runtime may contain tool output; review their retention separately. This hardening does not retroactively remove old logs or guarantee the absence of malicious code in third-party dependencies.

The hardened Windows browser-extension runtime is the tested target. Other launchers and the separate desktop application have not been certified for this authentication protocol. The source tests do not replace a live browser integration smoke test.

## Recovery

An incorrect pairing token can be replaced through Pair Bridge. To change the trusted extension ID, stop the bridge and remove only the trusted-extension-id file from the local SnapgentSecurity directory, then run start.bat. Do not share or expose the bridge-token file. Close the bridge when it is not needed.
