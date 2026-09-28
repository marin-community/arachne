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

A session that needs a person — a blocking question, a decision, or work ready for review — carries an `attention` or `blocked` flag and lands in the home's **Needs You** bucket. A tag-raised flag can be dismissed without opening the thread: the **✕** on any Needs You row, sidebar topic card, or thread-header **Dismiss** button clears it. Tool-approval attention (an unanswered permission request) has no dismiss — it is answered in the thread's approval prompt.

**Review diff** shows Loom's current change set with file hunks and line numbers. Worker threads offer **Integrate** into their topic; topic threads offer **Land** toward upstream. These buttons queue a structured request for the coordinating agent, which must still validate and report the actual Git result.

The **Resources** rail shows a topic's live set first — its repository, every thread's pull requests (state, review, CI), the GitHub issues its threads work, and the checkout — then durable attachments: design documents, files, PRs, and issues bound as versioned Loom artifacts on the topic branch. Attachments preview through Loom's server-side checkout and open the exact file in Zed; the binding survives an archived session (previewing a repo file then needs an active topic checkout). When a checkout is gone, its row recovers it via `repos.worktrees.ensure`. Agents keep the set current: claiming a GitHub issue with `loom issues add` puts it in the panel, and `skills/resources.md` tells agents to durably attach PRs and issues the topic should remember. [docs/design.md](docs/design.md) is the current Arachne design document.

Codex can use an existing ChatGPT subscription login on the runner. Run `codex login` as the same OS user that runs Loom, then verify with `codex login status`. API-key routes remain available when a key is configured. Arachne's bearer token authenticates to Loom; it is separate from Codex's inference login.

## Checks

```sh
npm run typecheck
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

Loom changes live in the sibling `../loom` checkout and follow its own build and test instructions. See [docs/handoff.md](docs/handoff.md) for the longer design and phased acceptance test.
