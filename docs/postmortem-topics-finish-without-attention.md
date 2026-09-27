# Postmortem: finished topics don't ask for attention

## What was expected

A topic that finishes its work — "i finished my work and the changes are
ready" — should surface as needing the user. Concretely: an
`attention` tag on the session, so Arachne's Needs You bucket and the
sidebar badge light up.

## What actually happened

The ctrl/cmd-enter-in-chat-view topic landed its work into `main`
(squash, commit 5769485), stamped `integration_result`, updated its
description to "Landed ctrl/cmd-Enter composer change into main as 5769485
(squash); typecheck+build+tests green" — and ended its turn as
`running` + `idle` with attention `ok`. Arachne correctly filed it under
the collapsed "Waiting / Resting" bucket. The user only noticed it was
done by reading the thread, and merged it themselves.

## Root cause

Not a UI bug — Arachne renders whatever attention state loom reports,
and loom faithfully reported `ok`. The failure was agent guidance:

The entrance note (loom-launch `entrance_note`), the builtin WEAVER.md
primer (weaver-core `BUILTIN_WEAVER_MD`), and the compaction replay all
described attention as a parenthetical:

> `(use attention or blocked when a person must act)`

A successfully-finishing agent reads "when a person must act" as
"nobody must act — the work is done." The rule it was trying to express
is: **any time the job isn't done and the next expected input comes
from the user — a blocking question, a decision, or work that is
finished and awaits review/integration — the session must flag
attention.**

## Fix (loom commit 08f553f)

All three guidance surfaces now carry an explicit dedicated bullet:

> Flag for attention: `loom status set --tag attention --message "..."`
> — do this whenever the next expected input is a person's: you are
> blocked on a question, a decision, or your work is done and ready for
> review (e.g. "changes are ready, PR #12"; then `--tag ok` only when
> you resume working). Use `--tag blocked` when you cannot proceed at
> all.

- `crates/loom-launch/src/provision.rs` — entrance note appended to every
  launch goal.
- `crates/weaver-core/src/agent.rs` — builtin WEAVER.md primer injected at
  SessionStart (the repo has no WEAVER.md of its own, so the builtin is
  what agents see).
- `crates/loom/src/cli/agent/session.rs` — post-compaction replay keeps
  the contract across context resets.
- `loom status set --tag` help text (cli + weaver-api schema doc).

Unit tests pin the contract at each site (entrance-note test, primer
test, compaction-replay test).

## Deployment note

The entrance note and primer are baked into the loom server binary. The
running local server (target/debug) needs a restart for new sessions to
receive the updated guidance.

## Verification performed

- `cargo test -p loom-launch --lib entrance_note` — ok
- `cargo test -p weaver-core --lib` — 126 passed
- `cargo test -p loom --test agent_cli` — 36 passed
- Pre-existing unrelated failures in the loom checkout (slack
  token-owner test, codex ACP supervisor timeouts) confirmed identical
  with changes stashed.
