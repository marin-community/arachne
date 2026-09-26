# Arachne

A macOS coding cockpit for [Loom](https://github.com/marin-community/loom). Loom is the authoritative control plane for sessions, worktrees, conversations, and attention; Arachne is a Tauri 2 and Vue 3 client.

## Run locally

```sh
npm install
npx tauri dev
```

The app initially targets `http://127.0.0.1:7878`. Open the connection indicator to set a different Loom URL. A remote Loom needs a bearer token; Arachne stores it in macOS Keychain. The web preview at Vite's port 5173 does not include Tauri commands and cannot connect to Loom by itself.

For the intended DGX Spark control plane, point Arachne at `http://100.121.8.110:7878` over Tailscale after Loom is deployed there. `scripts/deploy-loom-dgx.sh` describes the manual systemd installation. The Mac connects to that one control plane rather than maintaining a second Loom database.

## Launch and work on a task

Use the Inbox's task box to choose a repository, agent, inference profile, model, and reasoning effort before launch. The available profiles and model names come from the connected Loom server. Loom creates the session and checkout; Arachne streams the conversation and attention state. An idle ACP thread can switch its model through Loom's handoff operation. **Send to thread** delivers a note with source provenance into another thread's durable Loom channel.

The thread's resource strip shows the checkout and any associated pull request. **Open in Zed** opens a local checkout directly or a remote checkout over SSH, using the active session's path from Loom.

Codex can use an existing ChatGPT subscription login on the runner. Run `codex login` as the same OS user that runs Loom, then verify with `codex login status`. API-key routes remain available when a key is configured. Arachne's bearer token authenticates to Loom; it is separate from Codex's inference login.

## Checks

```sh
npm run typecheck
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

Loom changes live in the sibling `../loom` checkout and follow its own build and test instructions. See [docs/handoff.md](docs/handoff.md) for the longer design and phased acceptance test.
