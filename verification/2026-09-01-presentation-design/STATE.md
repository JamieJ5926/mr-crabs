# Presentation feature state, 2026-09-01 end of session

Nothing is committed. All work is in the working tree.

## Proven working, with evidence

Inline prompt. `--prompt-presentation inline` gives a normal terminal with the prompt
visible at launch and no dock chrome. Evidence `shots/01-inline-prompt.png`.

Art axis. Verified as text through the built-in path, which is the only path that
consumes these settings. `MR_CRABS_STARTUP_ART=apple` emits the Apple art, `native`
emits the parsed rustfetch logo, `none` emits no art. Evidence `shots/env-*.txt`.

Arrangement axis. `below` stacks system info under the art, `beside` gives two
columns, `hidden` gives art only. The info rows render at all for the first time;
every renderer previously discarded `FetchLine.info`.

Retained Apple logo after molt, in the one configuration tested.
Evidence `shots/02-molt-apple-retained.png`.

## BROKEN, and it is a regression introduced today

On the DEFAULT configuration the molt animation freezes part way and never completes.
The window stays black with a static cyan-outlined rectangle. Two captures five
seconds apart are byte-identical, `shots/10-default-a.png` and `10-default-b.png`,
md5 957699c7612c67935fd1eec7c604e568.

This did not happen at the 21:48 build, where launch showed a blank grid rather than
a frozen mask, so it was introduced by the art and retention lanes after that point.

One root cause was found and patched without fixing the runtime symptom.
`next_molt_deadline_ms` in `model/app_model.rs` used `(now_ms < end).then(...)`, which
returns None once the deadline is already overdue, so the scheduler cancels before the
completion tick can run. That patch is in the tree and its unit test passes. The
runtime freeze persists, so either the patch is incomplete or a second cause exists in
the retention path in `model/window.rs` and `ui/workspace.rs`.

## Recommended next step

Do not add more unit tests for this. Five separate lanes produced passing unit tests
while the runtime stayed broken, because the tests exercise `molt_layout` and the
deadline predicate directly rather than the scheduler rearm loop that actually halts.
Instrument the running binary: log every `arm_fetch_schedule` rearm, every
`tick_molt_animations` call, and each `StartupPresentation` transition, then launch with
defaults and read where the sequence stops. The safe fallback, if the cause is not
found quickly, is to disable molt art retention so the default path returns to the
pre-session behavior, keeping the three settings which are independently proven.
