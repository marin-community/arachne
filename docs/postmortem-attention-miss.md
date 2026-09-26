# Postmortem: asking the user a question didn't raise attention in Arachne

## What was expected

During the spinner task, the agent (me) asked the operator a blocking
question mid-turn — "are you building in the main checkout or a
worktree?" — that only a human could answer. The cockpit (Arachne)
should have surfaced that session as needing the user: an
attention/blocked tag on the session, live-updating badges, and (on the
attention-first home screen) a NEEDS YOU bucket.

## What actually happened

The question appeared only as ordinary transcript text. No attention
tag was set, so:

- the sidebar badge stayed the plain "run" chip (correctly — the tag
  never existed), and
- the session never entered any NEEDS YOU view.

The operator had to read the thread to notice the question.

## Root causes (two independent, both real)

### 1. Agent-side: I never raised attention — the process gap

Loom's contract is that **the agent must self-report** needing a human.
Nothing in the stack infers it:

- `loom status set --tag attention|blocked` is the only path that sets
  the `attention` tag; it is agent-initiated by design (loom-store
  `status.rs`: the only automatic writers are lifecycle edges —
  `working` CLEARS attention and sets running, `idle` sets the quiet
  idle mark; the only automatic attention writer is a turn-budget cap
  on automation-class sessions, which never applies to interactive
  topics).
- The session's orientation note says exactly when to use it: "use
  `attention` or `blocked` when a person must act". I asked a question
  at the end of a turn and ended the turn without ever running the
  command.

So the first failure is mine: **the moment I ask a blocking question, I
must run `loom status set --tag attention --message "<the question>"`**
before ending the turn. (Done now for this session, with the question in
the message.)

There is a subtler behavioral trap worth recording: a question asked at
the very end of a turn races the turn-end lifecycle edge. Turn start
(`working`) CLEARS the attention tag; my turn had already started, so a
tag set mid-turn would have held — but a question asked early in a long
turn, tagged immediately, then followed by... nothing that clears it, is
the safe pattern. The tag survives across turns until someone lowers it
(`--tag ok`), so "tag it the moment you ask, untag when answered" is the
right discipline, and `--tag ok` never wipes the message.

### 2. Arachne-side: the attention UI is not on main

The cockpit features that would have DISPLAYED the tag — even had I set
it — live only on the unmerged branch `weaver/could-you-review-the-status-of-a`:

- `4c36cc8` "Attention-first home screen" — `HomeView.vue`, the
  NEEDS YOU / WORKING / RESTING home (never entered `origin/main`;
  `git merge-base --is-ancestor 4c36cc8 origin/main` is false).
- Main has the earlier layers only: tag badges in the sidebar
  (`cf23815`) and fleet live-update on tag events (`c4cb8fa`). Both
  verified working — badges flip within seconds of a tag change (the
  earlier session `mi1llejb` shows `attention: blocked` rendering
  correctly in the fleet list).

So even a perfectly-behaved agent asking a question would today only
get the sidebar badge, not the home screen's answer to "what needs me?"

## Fixes

1. **Behavioral (done)**: `loom status set --tag attention --message
   "worktree question answered; spinner shipped"` for this session, and
   the discipline is now explicit: raise attention when asking a
   blocking question; lower to `ok` (keeps the message) when answered.
2. **Loom-side guardrail (proposal, upstream)**: an agent turn that
   ends with a question mark in its final agent message, or an ACP
   `end_turn` following a turn whose journal contains no user message,
   could nudge/flash an attention hint server-side — but it must remain
   advisory, since only the agent knows whether it can proceed alone.
3. **Arachne-side (next PR)**: merge/rebase `HomeView.vue` (4c36cc8)
   onto main so the NEEDS YOU bucket exists in the shipped app. It
   applies cleanly (App.vue diff is additive: HomeView import + an
   `v-else` pane; the only conflicts are the Keychain token storage
   from 3c9eeca, which is independent).

## Verification trail

- Tag mechanism works end-to-end today: session `mi1llejb` carries
  `attention: blocked` and renders in Arachne's fleet badges
  (live-update wired by c4cb8fa, on main).
- `loom status set --tag ok/attention/blocked` maps to the `sessions.status.set`
  op, which writes the description + tag + one `tag` event atomically
  (loom `cli/agent/status.rs`, `web/sessions.rs`).
- Nothing sets attention automatically except the automation turn cap
  (loom-store `status.rs` `lifecycle_mutations` + cap path).
- HomeView absent on main: `git show origin/main:src/components/HomeView.vue` fails;
  present on `weaver/could-you-review-the-status-of-a` (4c36cc8).
