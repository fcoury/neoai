# neoai (Tauri + React + TypeScript)

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## NeoAI ACP Agent Fallback

When `codex-acp` is not available on `PATH`, neoai can install a managed copy from
`zed-industries/codex-acp` releases (currently pinned to `v0.9.2`) and start it automatically.

- macOS install root: `~/.neoai/agents/codex-acp/<version>/`
- Other platforms: app-local data directory under `agents/codex-acp/<version>/`
