# Snapgent

A browser-based local agent harness that connects supported AI chats to your filesystem, terminal, Git and optional MCP integrations through a native Rust bridge.

## Architecture

AI chat → Browser extension → Authenticated loopback bridge → Selected local workspace or optional MCP integration.

No running code editor is required. Choose an absolute local folder before starting a workspace session. Each session is bound to its own workspace.

## Features

- Native Rust bridge with explicit workspace selection.
- Filesystem, search, editing, terminal and Git tools.
- Shared automatic submission retry policy across 13 AI providers.
- Session isolation, steering and optional MCP integrations.
- Local authentication and privileged-operation approval dialogs.

## Installation (Windows)

1. Build the native executable using `powershell -File development/build-native.ps1`.
2. Load `snapgent-extension` as an unpacked extension in your Chromium browser.
3. Run `start.bat` and configure the trusted extension ID in the native application. A fresh clone needs no `config.json`; optional MCP integrations create it when configured.
4. Copy the pairing credential from Settings in the visible desktop UI and paste it into the extension popup. Existing saved credentials continue working.
5. Keep the desktop window open while using the agent. Stop bridge or close the window to disconnect all sessions.
6. Open a supported AI chat, select Local Workspace, choose your absolute folder path and press Start.

### Upgrading from an older bridge

Close any old Snapgent windows or tray instances first. If a previous headless executable is still listening, check its process path and command line in Task Manager and stop only that known Snapgent instance before rebuilding. The new extension rejects legacy bridges instead of displaying them as connected. The new executable has no headless listener or hide-to-tray mode; legacy CLI flags open the UI.

## Supported providers

ChatGPT, Claude, DeepSeek, Gemini, Grok, Kimi, GLM, Qwen, Qwen Coder, Arena, Meta AI, Ptero and Mistral. Provider-specific website changes can affect compatibility.

## Security

The bridge uses loopback connections and authenticated sessions. Shell commands and optional MCP executables are not OS-sandboxed. Tool results submitted to an AI chat are transmitted to that provider. See [SECURITY.md](SECURITY.md).

## Development

Run `cargo test --manifest-path rust/Cargo.toml --workspace --offline` to test the native bridge. The extension test scripts are in `snapgent-extension/`.

Legacy Python and Electron implementations remain in the repository for reference and are not the default native runtime.

## Credits and license

Derived from [herdodp/snapgent-connector](https://github.com/herdodp/snapgent-connector). Original authorship and third-party notices are retained. Licensed under [GPL-3.0](LICENSE).
