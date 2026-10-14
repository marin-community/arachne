# Arachne

A macOS coding cockpit for [Loom](https://github.com/marin-community/loom). Loom is the authoritative control plane for sessions, worktrees, conversations, and attention; Arachne is a Tauri 2 and Vue 3 client.

## Run locally

```sh
npm install
npx tauri dev
```

The app initially registers `http://127.0.0.1:7878`. Open the connection indicator to register other Loom backends and choose the default for new projects and ungrouped tracks. All registered backends appear together; each project belongs to the backend where it was created. A remote Loom needs a bearer token; Arachne stores a separate credential for each backend in macOS Keychain. The web preview at Vite's port 5173 does not include Tauri commands and cannot connect to Loom by itself.

To run both Loom and the native app locally, clone Loom into `../loom`, install
Node 22+, Rust, and the platform's Tauri prerequisites, then run:

```sh
npm ci
npm run dev:local
```

The launcher builds Loom and Tapestry, seeds the local operator from your `gh`
login, and starts Loom at `http://127.0.0.1:7878`. Its database and server log
live in `.local/loom-state/` (override with `WEAVER_HOME`). It reuses an existing
server on that port and stops only the server it started when the app exits.
Set `LOOM_SRC` for a different checkout, or `LOOM_OWNER_GITHUB` and
`LOOM_OWNER_GITHUB_ID` to supply the bootstrap identity without `gh`.

On Ubuntu, the native app needs `build-essential`, `pkg-config`,
`libgtk-3-dev`, `libwebkit2gtk-4.1-dev`, and `libdbus-1-dev`. Saved tokens use the
desktop Secret Service on Linux. The launcher also recognizes project-local
Node and WebKit prerequisites under `.local/`.

For a personal Spark deployment over Tailscale, run
`scripts/setup-loom-spark.sh` on the Spark with a Loom checkout at `~/src/loom`
(or set `LOOM_SRC`). It installs Node and the Codex ACP adapter under
`~/.local/share/loom`, builds Loom, and enables a `systemctl --user` service
bound to the machine's Tailscale IP. It also configures the existing `gh`
account token and, on first boot, the default Codex profile. The saved Arachne
API token stays in `~/.weaver/arachne-framework1-token`; it is never printed.
Git runs without interactive prompts; SSH remotes use the runner's on-disk
key rather than waiting for approval from a desktop SSH agent.
Use `systemctl --user status loom` and `journalctl --user -u loom` on the Spark
to inspect the service. In Arachne, the header's connection indicator opens
the saved-server dropdown for switching between local Loom and Spark.

For the intended DGX Spark control plane, point Arachne at `http://100.121.8.110:7878` over Tailscale after Loom is deployed there. `scripts/deploy-loom-dgx.sh` describes the manual systemd installation. Register that backend alongside a local Loom to see both fleets together.

## Launch and work on a task

**+ Project** opens a full pane in the chat area with a project name,
repository/base defaults, and resource bindings. Add the resources that new
tracks should inherit, then choose **Create project**. The pane stays open as
Project settings for further edits; each project's **⚙** button opens it later.

Use **New thread** for a one-off, or **New track** for a durable effort led by a coordinator. New track opens a chat immediately: the setup choices (title, repository, seed base, editable topic branch, inherited project resources) sit in a setup card above the chat box and disappear once the model starts — the first send delivers the message and launches the track. The chat box keeps message text, attachments, @-mentions, and the launch-config pills (profile, agent, model, effort). **Convert to track** preserves a thread's conversation and checkout. Choose a managed repository or local checkout path, base branch, agent, inference profile, model, and reasoning effort before launch. Choose the backend in the new-project or new-track setup card. Its repository, profile and model choices come from that backend. Creating a track from a project heading selects the project’s backend automatically. Loom creates the session and checkout; Arachne streams the conversation and attention state. An idle ACP thread can switch its model through Loom's handoff operation. **Send to thread** delivers a note with source provenance into another thread's durable Loom channel.

Type **@** in a chat to complete repository file paths, including untracked files outside the ignore rules. Before launching a track, completion uses the selected repository; for a remote Loom server it needs an existing session checkout in that repository. Use **@{** to browse attached resources instead. Pick a file with **Tab** or **Enter**; **⌘/Ctrl+Enter** sends the message and **Shift+Enter** adds a newline.

While the agent is working, an empty composer shows **Stop** in place of Send. Typing a message or attaching files brings Send back so you can queue a follow-up. To restore an archived track, enable **Archived** in the Tracks sidebar and click **Unarchive** on its card or in its thread header. Loom restores the checkout and resumes the agent with the existing conversation.

The thread's resource strip shows the checkout and any associated pull request. **Open in Zed** opens a local checkout directly or a remote checkout over SSH, using the active session's path from Loom.

A session that needs a person — a specific decision or access request that its hierarchy cannot resolve — carries an `attention` or `blocked` flag and lands in the home's **Needs You** bucket. A tag-raised flag can be dismissed without opening the thread: the **✕** on any Needs You row, sidebar track card, or thread-header **Dismiss** button clears it. Tool-approval attention (an unanswered permission request) has no dismiss — it is answered in the thread's approval prompt.

**Review** in the inspector shows selected declared Changes with file hunks and
line numbers. Threads produce zero, one or multiple outputs: successful review,
research or coordination with no produced Change needs no Land. The inspector's
**Landing** queue contains explicit Changes with stable IDs, pinned scopes and
independent outcomes. Select one to review or Land; one click queues a working
output's wrap-up behind active work and asks the coordinator to complete review,
repair, commit, validation and acceptance. Completed ranges reuse their original
completion without resuming the source. Default acceptance is a squash into
the canonical topic branch. Optional **Prepare** remains preparation only.
Later outputs from the same Thread remain eligible, and historical completions
never reappear as new deltas. See [Change v1 contract](docs/thread-landing-contract.md).
New Tracks use editable `arachne/tracks/<slug>` accepted refs, distinct from Loom
scratch; legacy Tracks retain their accepted ref. **Autoland** policy lives in
the inspector, defaults Off, and uses actual successful explicit-output result
wakeups with live policy guards. Chat has no autoland controls. Coordinator
**Publish** remains a separate explicit chosen action on accepted Track state:
Single PR, genuine PR stack, or the chosen local/upstream operation. Queued
requests are not completed results.

The **Resources** rail shows a track's live set first — its repository, every thread's pull requests (state, review, CI), the GitHub issues its threads work, and the checkout — then durable attachments: design documents, files, PRs, and issues bound as versioned Loom artifacts on the track branch. Attachments preview through Loom's server-side checkout and open the exact file in Zed; the binding survives an archived session (previewing a repo file then needs an active track checkout). When a checkout is gone, its row recovers it via `repos.worktrees.ensure`. Agents keep the set current: claiming a GitHub issue with `loom issues add` puts it in the panel, and `skills/resources.md` tells agents to durably attach PRs and issues the track should remember. [docs/design.md](docs/design.md) is the current Arachne design document.

The review panel can request an agent review or prepare changes for landing. Its file picker and diff rows open a small local text editor with line numbers, wrapping, Tab indentation, and **⌘S**. Saves check whether the file changed on disk; Land includes inspection, validation and committing intended editor changes while preserving unrelated edits. Review preparation does not authorize acceptance or publication. Review, Land and Publish requests remain distinct from completed results.

Track and thread rows show a spinner whenever that thread or any descendant is working, including collapsed descendants. Quiet threads omit activity badges; human attention appears as a yellow exclamation box. The **Threads** inspector keeps advisory ready, conflict/stale, parent-help, waiting, and landed facts. New chat input clears the landed tag. Worker problems go to their parent first; Needs You and OS notifications require an explicit attention/blocked tag or a live tool approval.

**Launch reviewer** opens a fresh child Thread against the candidate branch with a bounded objective and target reference. It receives role guidance and no implementation transcript; preparation requests still go to the original worker. The **Handoffs** inspector collects durable typed results separately from routine mailbox traffic and links to their source Threads. Coordinator, worker, and reviewer instructions live in `skills/` and travel through Loom launch guidance and the durable first turn, including on older Loom servers. Coordinators maintain a compact branch-scoped `track-brief` artifact and use risk-based review rather than mandatory lifecycle gates. [docs/workflow-design.md](docs/workflow-design.md) describes the coordination policy and later automation priorities.

The **Activity** inspector displays Loom's existing watches, delivery subscriptions, durable messages/results, and delivery errors. It refreshes on fleet events and explicitly on demand; channel/watch-only changes need Refresh until Loom exposes those events to this client. No agent polling loop is added. Attention banners show concrete requests and link directly to the affected worker or coordinator. Hiding a banner leaves its request in Needs You.

**⌘K** searches titles, descriptions, branches, repository names, and PR metadata, including archived work. **⌘N** opens a one-off thread; **⌘⇧N** a track; **⌘⇧D** delegates; **⌘⇧E** toggles the inspector; **⌘⇧H** goes home; **⌘,** opens saved Loom servers. Ctrl works in place of Cmd. Repository/base defaults and drafts are scoped by backend; each backend’s bearer token lives in its own Keychain entry. A disconnected backend retains dimmed project headings with an **Unavailable** label and reconnect action, including after restart when project metadata was cached. Its tracks are omitted from actionable fleet views until recovery; other backends remain usable. Reconnecting refreshes only that backend.

Codex can use an existing ChatGPT subscription login on the runner. Run `codex login` as the same OS user that runs Loom, then verify with `codex login status`. API-key routes remain available when a key is configured. Arachne's bearer token authenticates to Loom; it is separate from Codex's inference login.

## Checks

```sh
npm test
npm run typecheck
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Loom changes live in the sibling `../loom` checkout and follow its own build and test instructions. See [docs/handoff.md](docs/handoff.md) for the longer design and phased acceptance test.
