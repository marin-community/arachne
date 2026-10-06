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

Use **New thread** for a one-off, or **New track** for a durable effort led by a coordinator. New track opens a chat immediately: the setup choices (title, repository, base branch, inherited project resources) sit in a setup card above the chat box and disappear once the model starts — the first send delivers the message and launches the track. The chat box keeps message text, attachments, @-mentions, and the launch-config pills (profile, agent, model, effort). **Convert to track** preserves a thread's conversation and checkout. Choose a managed repository or local checkout path, base branch, agent, inference profile, model, and reasoning effort before launch. The available profiles and model names come from the connected Loom server. Loom creates the session and checkout; Arachne streams the conversation and attention state. An idle ACP thread can switch its model through Loom's handoff operation. **Send to thread** delivers a note with source provenance into another thread's durable Loom channel.

The thread's resource strip shows the checkout and any associated pull request. **Open in Zed** opens a local checkout directly or a remote checkout over SSH, using the active session's path from Loom.

A session that needs a person — a blocking question, a decision, or work ready for review — carries an `attention` or `blocked` flag and lands in the home's **Needs You** bucket. A tag-raised flag can be dismissed without opening the thread: the **✕** on any Needs You row, sidebar track card, or thread-header **Dismiss** button clears it. Tool-approval attention (an unanswered permission request) has no dismiss — it is answered in the thread's approval prompt.

**Review** in the inspector and **Review diff** in the header show Loom's current change set with file hunks and line numbers. Worker threads offer **Integrate** into their track; coordinator threads offer **Land** toward upstream. These buttons queue a structured request for the coordinating agent, which must still validate and report the actual Git result.

The **Resources** rail shows a track's live set first — its repository, every thread's pull requests (state, review, CI), the GitHub issues its threads work, and the checkout — then durable attachments: design documents, files, PRs, and issues bound as versioned Loom artifacts on the track branch. Attachments preview through Loom's server-side checkout and open the exact file in Zed; the binding survives an archived session (previewing a repo file then needs an active track checkout). When a checkout is gone, its row recovers it via `repos.worktrees.ensure`. Agents keep the set current: claiming a GitHub issue with `loom issues add` puts it in the panel, and `skills/resources.md` tells agents to durably attach PRs and issues the track should remember. [docs/design.md](docs/design.md) is the current Arachne design document.

The review panel can request an agent review or prepare changes for landing. Its file picker and diff rows open a small local text editor with line numbers, wrapping, Tab indentation, and **⌘S**. Saves check whether the file changed on disk; uncommitted editor changes must be committed/validated before local landing. Review requests and landing requests remain distinct from completed results.

The **Activity** inspector displays Loom's existing watches, delivery subscriptions, durable messages/results, and delivery errors. It refreshes on fleet events and explicitly on demand; channel/watch-only changes need Refresh until Loom exposes those events to this client. No agent polling loop is added. Attention banners show concrete requests and link directly to the affected worker or coordinator. Hiding a banner leaves its request in Needs You.

**⌘K** searches titles, descriptions, branches, repository names, and PR metadata, including archived work. **⌘N** opens a one-off thread; **⌘⇧N** a track; **⌘⇧D** delegates; **⌘⇧E** toggles the inspector; **⌘⇧H** goes home; **⌘,** opens saved Loom servers. Ctrl works in place of Cmd. Repository/base defaults and drafts are scoped by server; each server's bearer token lives in its own Keychain entry.

Codex can use an existing ChatGPT subscription login on the runner. Run `codex login` as the same OS user that runs Loom, then verify with `codex login status`. API-key routes remain available when a key is configured. Arachne's bearer token authenticates to Loom; it is separate from Codex's inference login.

## Checks

```sh
npm test
npm run typecheck
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

Loom changes live in the sibling `../loom` checkout and follow its own build and test instructions. See [docs/handoff.md](docs/handoff.md) for the longer design and phased acceptance test.
