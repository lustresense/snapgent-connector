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
3. Run `start.bat` and configure the trusted extension ID in the native application.
4. Pair the extension with the bridge locally.
5. Open a supported AI chat, select Local Workspace, choose your absolute folder path and press Start.

## Supported providers

ChatGPT, Claude, DeepSeek, Gemini, Grok, Kimi, GLM, Qwen, Qwen Coder, Arena, Meta AI, Ptero and Mistral. Provider-specific website changes can affect compatibility.

## Security

The bridge uses loopback connections and authenticated sessions. Shell commands and optional MCP executables are not OS-sandboxed. Tool results submitted to an AI chat are transmitted to that provider. See [SECURITY.md](SECURITY.md).

## Development

Run `cargo test --manifest-path rust/Cargo.toml --workspace --offline` to test the native bridge. The extension test scripts are in `snapgent-extension/`.

Legacy Python and Electron implementations remain in the repository for reference and are not the default native runtime.

## Credits and license

Derived from [herdodp/snapgent-connector](https://github.com/herdodp/snapgent-connector). Original authorship and third-party notices are retained. Licensed under [GPL-3.0](LICENSE).
