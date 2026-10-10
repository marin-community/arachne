# Thread Land / Track Publish contract

Land accepts a worker Thread's intended changes into its same-repository Track canonical topic branch. Publish is a separate explicit root/Track action toward PRs or an external/local destination. Autoland never publishes.

## Durable branch tags (Loom metadata)

- `topic=true`: Track coordinator marker (existing).
- `track_topic_branch=<valid local branch ref>` on the Track coordinator: canonical accepted ref, distinct from its Loom-managed scratch/session branch. Missing tag on a legacy Track means its existing coordinator branch; never silently migrate legacy work.
- `track_autoland=on|off` on the Track: OFF when absent/invalid.
- `thread_autoland=inherit|on|off` on sources: INHERIT when absent/invalid. ON overrides Track OFF; OFF overrides Track ON. Only complete successful intended results qualify for automatic acceptance; checkpoint/help/partial results do not.
- `landing_request=pending` on a source may record advisory queued wrap-up/acceptance. It is neither readiness nor a successful result.
- `integration_result` remains the compatibility signal for actual successfully accepted work; `integration_state`/`integration_ready` remain advisory reported state, never inferred from sleeping/working/completion.
- Track publishing records `publish_result` after actual successful publish, separate from worker acceptance outcomes.

## Canonical ref creation

New Tracks have an editable topic-branch field; empty means automatic `arachne/tracks/<slug>` inferred from explicit title or launch title/task. Draft persists the editable value. Validate explicit local branch refs before launch; reject invalid/reserved ref forms. Disambiguate duplicate defaults and reject explicitly occupied/claimed unrelated refs, preserving existing branches. Initialize the REAL Git canonical ref from the fresh Track seed revision (launch HEAD, before substantive work), through an explicit durable initialization instruction embedded in the launch prompt; safe local creation is optional. Never rename a live Loom branch or change primary HEAD. Tags must agree with the created ref. Remote initialization must do real Git work and report its result; display metadata alone is not initialization. Canonical initialization is required before any acceptance/publishing. Legacy Track without a tag retains its existing branch.

## Backend commands consumed by frontend

- `launch_session`: existing arguments plus `topicBranch: string|null` (Rust `topic_branch: Option<String>`). Used only for a new root Track (`asTrack=true`). Empty/omitted requests automatic inference; nonempty is explicit user branch.
- `land_thread`: `{sessionId, strategy}` with `squash` default; allowed acceptance choices squash, merge, rebase, cherry-pick (ask may remain compatibility). Returns existing IntegrationTarget `{coordinator_id,target_branch,coordinator_name}`; optional warnings may describe delivery limitations. `integrate_session` may remain an alias with identical acceptance semantics. No PR/push/local-main acceptance strategy.
- `set_autoland`: `{sessionId, policy}` where a Track accepts on/off and a source accepts inherit/on/off. Returns refreshed SessionView. Persists Loom tag, ensures real delivery subscriptions for existing nested descendants, sends explicit complete coordinator policy instructions through existing durable delivery. Disabling policy requires stale automatic requests to reread LIVE Track/source tags immediately before writing and stop when effectively OFF. New child launches under enabled policy arrange route/notify coordinator as part of launch setup.
- `publish_track`: `{sessionId,strategy,upstream?:string|null}` returning LandResult `{local,landing?}`. Strategy wire values open-pr (Single PR), pr-stack (PR stack), land-locally (squash into local primary current branch), squash/merge/rebase/push where appropriate. Always source the canonical accepted topic branch. Existing `land_topic` may remain a compatibility wrapper, with these publishing semantics. Local accepted-ref publishing preserves foreign edits and clean synchronous behavior; coordinator scratch dirt is not part of accepted source. Queued publish returns local=false and no premature completion. Publishing requires this explicit chosen user action; no autoland path invokes it.

## Delivery and prompts

Manual thread Land sends an explicit source wrap-up prompt behind an active ACP turn (`queue_prompt`, send_now=false), preserving original source id/repo/branch/worktree. It also sends one complete coordinator acceptance request: wait for successful intended wrap-up result, inspect/review/repair, commit intended source changes while preserving foreign edits, validate, reverify exact source and canonical target, apply chosen acceptance, and report actual outcome. Never interrupt active work or ask for a second Prepare click. Terminal sources/coordinators use durable channel messages (`send_to_thread`) instead of typing into an active terminal or claiming an ACP queue.

Use real existing Loom `POST /api/channels/subscription/set` delivery records. Direct child results already link/wake immediate parent. For nested candidates, subscribe canonical coordinator (session subject) in deliver mode to the IMMEDIATE-PARENT channel that receives linked result notices; verify returned channel_id, subject_kind=session, subject_id and mode. Ensure coordinator own-channel route as needed. Manual acceptance and autoland policy setup establish routes before claiming delivery/going idle. Future nested launch setup must establish the same route when effective autoland is ON. Use existing result wakeups/instructions; no scheduler/polling loops.

Publishing prompts clearly identify canonical source and any source checkout (which may differ from coordinator scratch). Stack mode must derive an ordered list from accepted history/branches, explicit dependent PR bases and head refs; validate each dependent change, create actual separate PRs and report ordered links/results. It may not pretend one PR is a stack.

Frontend helper `src/trackPolicy.ts` should centralize tag lookup, canonicalTopicBranch, same-repository Track resolution, effective autoland, branch inference/validation, and share these semantics across queue, ThreadView and review requests. Backend helper module mirrors this contract with cross-language fixture tests where practical. Review compares against canonical ref, never incidental scratch or main.
