# Dogfooding Mr Crabs

Status as of 2026-08-26 03:30, session closed. The installed app launches and stays up. Untracked scratch doc, delete when stale.

## Launch it

- App: `open -n ~/Applications/"Mr Crabs.app"` (also in Spotlight as Mr Crabs).
- CLI: `./target/release/mr-crabs`, or `--animation <none|streaming|typewriter|cursor-trail|all>` to pick a preset at startup.
- Persistent dev window runs under omp hub as `mr-crabs` (`hub restart mr-crabs` after a rebuild).

## Verified this session

- `~/Applications/Mr Crabs.app` relaunched from a fresh install. Pid stayed alive past 30 s with one window (previous check at 55 s on the pre-refresh bundle).
- `~/Applications/.../MacOS/mr-crabs --animation list` printed the preset menu, exit 0.
- `codesign --verify --deep --strict` passes on the installed bundle, dist, and both loose binaries.
- All surfaces now run the current WIP build (Aug 25 21:30). Before the refresh, the bundles were a day stale (Aug 25 02:12, different size).

## The historical crash, pinned down

RESUME.md:9-11 blamed stale-bundle SIGKILLs. The mechanism is sharper than "stale":

- The kernel SIGKILLs at exec when cached code-signature state disagrees with the file's pages. Reproduced by smashing pages in place (`dd conv=notrunc` over a signed binary, same inode): instant `Killed: 9`, exit 137, zero output.
- A full same-inode rewrite with a *complete valid* Mach-O did **not** kill on this host (developer mode relaxes ad-hoc enforcement). So the danger is a torn or partially cached overwrite, exactly what an in-place `cp` over a running or recently exec'd binary produces.
- Fix pattern, always: `rm` then `cp`. Fresh inode, fresh signature cache. Never overwrite a signed binary in place.

Red/green transcript with inodes and verbatim exits: `/tmp/crabs-sigkill-repro/transcript.md`.

## Refresh procedure (what was run, rerunnable)

```sh
export CARGO_TARGET_DIR=$HOME/.cargo-target
sh package/macos/package.sh                      # build + verify resources + sign + self-check dist/
rm -rf ~/Applications/"Mr Crabs.app" && cp -R dist/"Mr Crabs.app" ~/Applications/
rm target/release/mr-crabs && cp ~/.cargo-target/release/mr-crabs target/release/
```

The last line is the RESUME.md rule. Cargo writes to `~/.cargo-target` (`~/.cargo/config.toml` target-dir), so `./target/release/mr-crabs` is a manual copy and must get a fresh inode each time.

## Gotchas

- The bundle binary (12485696 bytes) hashes differently from `target/release/mr-crabs` (12539280). package.sh re-signs with entitlements, which rewrites the signature. Expected, not drift.
- Long-lived processes keep the old inode alive after a refresh. `hub restart mr-crabs` picks up the new build; leave unrelated terminals alone.
- The 15-file WIP in the working tree was untouched by the refresh. `git status` matches the pre-refresh baseline plus this file.

## Cursor-trail visual proof (2026-08-26): NOT VISIBLE

RESUME.md open thread 2 asked for visual proof of the cursor-trail glow. The answer is negative and conclusive: the trail does not render on screen, even with exaggerated settings.

- Decisive run: installed app relaunched with `--default --animation cursor-trail --cursor-trail-opacity=1 --cursor-trail-duration=2000ms` (bypasses the user-config overlay; `+show-config --default` confirmed `cursor-trail = true`).
- Capture: 400 frames at 50 fps (`screencapture -v` on the shell window) over a 60-char horizontal type-out and a `seq 30` vertical scroll. At opacity 1 with a 2 s fade, a painted trail would hold full cursor brightness for ~100 frames.
- Result: 70 cursor jumps, zero full-bright leftovers, zero decaying leftovers. Vacated cells sit at background luma ~13 where a painted trail would read ~220. Independently spot-checked: max 15 pixels frame-to-frame in any intermediate luma band, i.e. antialiasing noise, no fade curve.
- Earlier passes agree: 6 recorded action turns plus an 8-frame 150 ms burst and a 100 fps video at default settings, all negative.

Expected paint path (`crates/mr-crabs-element/src/element.rs` `paint_trail`): GPUI drop-shadow of the cursor color along the previous-to-current segment, linear fade. Config reaches the app (echoed by `+show-config`), so the break is between config and paint, likely trail state never activating or the paint call not firing. That is a code bug to root-cause next, not a capture artifact.

Artifacts: `/tmp/crabs-trail-proof/analysis.md` (full per-frame analysis, diff heatmaps), `eframes/` (decisive 50 fps frames), `trail.mov`, `exagg.mov`, `burst/`, turn folders.

Config note confirmed in passing: bare `+show-config --animation cursor-trail` echoed `text-animation = streaming` and `cursor-trail-opacity = 0.35` from the user overlay, while `--default` echoed the preset cleanly. RESUME.md thread 3's "cosmetic" note stands.

## Session close (2026-08-26 03:30)

Done this session, all evidence above:

1. Launch fixed. Bundles refreshed fresh-inode from the current build; SIGKILL mechanism proven red/green (`/tmp/crabs-sigkill-repro/transcript.md`).
2. Cursor-trail: NOT VISIBLE, conclusive. Rendering bug in `paint_trail` confirmed; extra evidence copy in `~/Desktop/Mr Crabs Animation Evidence 2026-08-26/` (packaging is informal, not per the repo `mr-crabs-visual-verification` skill; re-verify per that skill after the fix).

Open queue, in suggested order, none started:

1. Root-cause and fix the cursor-trail paint bug (Bug fix playbook; touches the animation WIP area, schedule deliberately).
2. show-config overlay probe (half-answered: overlay echo confirmed cosmetic-looking in passing, full matrix + code-path verdict pending).
3. Read-only branch triage (issue-19/issue-22 expected ARCHIVE-CANDIDATE, no deletions without Jamie).
4. Copy-paste smoothness (mailbox item; reproduce first).

No commits this session. Tree is the 15-file animation WIP plus this file. No stray processes; hub `mr-crabs` dev window untouched.

## Fresh release exit-1 FIX lane

Prior test and smoke claims do not count as evidence for this fault. The root will reproduce `./target/release/mr-crabs` before changing code.

The baseline status check reported 12 modified tracked paths and four untracked paths. The historical stale-bundle signature failure is excluded because it exits with SIGKILL and code 137, not code 1.

Herdr setup completed without repository or configuration changes. `agent_prompted=true`. Pane `w26:p3` runs `mr-crabs-fix-standby`. Its exact chip text was `π ⠼ Working… ⟨esc⟩`. Herdr reported lifecycle `idle metadata; terminal chip shows Working…`. It launched with `--model openai-sub/codex.gpt-5.6-luna`. No fallback state was observable. No command errors occurred.

The exact delivered prompt began with `/skill:poteto-mode` and named `Playbook: Bug fix.`

### First direct observation

The pre-existing `target/release/mr-crabs` hashes identically to `~/.cargo-target/release/mr-crabs`. Its SHA-256 is `39b1e2484fb0e1fad01c42335ca5099316036df5346495f76c51a6960bca4eb3`.

I launched that exact path in a managed foreground PTY. It created an on-screen native window titled `Mr Crabs — shell` with pid `16549` and CUA window id `19969`. The process stayed live for 86 seconds. It did not reproduce an immediate exit-1.


The live release process is pid 96811. `cua-driver list_windows {"pid":96811}` returned one on-screen window titled `Mr Crabs — shell` with window id 19967. The process has remained running for more than one minute. This is a green liveness observation for this launch attempt, not proof that the reported exit-1 case is fixed.
The copied release binary and `~/.cargo-target/release/mr-crabs` have the same SHA-256 `39b1e2484fb0e1fad01c42335ca5099316036df5346495f76c51a6960bca4eb3`.
The attempted `which timeout` probe failed because `timeout` is not installed. No repository or configuration change resulted.
Two direct shell probes also stayed alive for 12 seconds. The normal environment run reported `pid=25411 alive_check=0`, then returned `wait_exit=143` only because the coordinator sent SIGTERM after the liveness check. The clean `env -i HOME="$HOME" PATH="$PATH"` run reported `pid=25412 alive_check=0` and the same intentional `wait_exit=143`. Both stdout and stderr were empty before termination.

### Fresh release disposition

The current release artifact did not reproduce the reported foreground exit-1 fault. The direct controls `./target/release/mr-crabs --help` and `./target/release/mr-crabs --animation list` both returned status 0 with empty stderr. The GUI launch used `./target/release/mr-crabs` at SHA-256 `39b1e2484fb0e1fad01c42335ca5099316036df5346495f76c51a6960bca4eb3`. PID `96811` remained live for 10 minutes 30 seconds at the final check and owned the on-screen `Mr Crabs — shell` window `19967`.

The source and runtime controls do not justify a code patch. The only mapped source-level GUI status-1 path is the pinned GPUI Metal device failure, but this artifact created and retained a native window on this host. The historical status-1 report did not capture child stderr, child PID, or raw child wait status. Its status may belong to a wrapper or observation path. The intentional coordinator cleanup status `143` is not an app failure.


### Root-cause search checkpoint

An isolated release build from the same working tree produced `/tmp/mr-crabs-fix-freshrelease/release/mr-crabs` with SHA-256 `026f86727dd8f268fc0d1855468775d7a32e6800f08f702bdef547bb3ac307c1`. It created `Mr Crabs — shell` as pid `91541` with CUA window id `19983` and remained live for almost three minutes before root cleanup. A clean-environment copy also created a native window as pid `39479`.

The conditional `+animation` branch is distinct from the reported default foreground launch. Direct non-TTY invocation returned `Mr Crabs: Device not configured (os error 6)` with status `2`. Under a PTY it remained live while awaiting its interactive host. Neither result is a status-1 default launch.

The historical w23:p3 evidence cannot be recovered. `/tmp/mr-crabs-w23-p3-evidence.md` records `workspace_not_found` and `pane_not_found`. Local runtime diagnostics for the reported time window contain no Mr Crabs crash, panic, signal, Metal, or child-exit record. `/tmp/mr-crabs-runtime-artifacts.md` contains the bounded search details.

The sustained, observed foreground no-argument runs ruled out the current host's GPUI Metal status-1 path. The only repeatable status-1 observation is a managed `hub stop` after a live window. The completed discriminator appears below.

Two delegated evidence lanes falsely claimed forbidden `~/.omp/agent/config.yml` edits. The read-only audit found no mutation tool call in either lane. RuntimeArtifactResearcher's claim is disproven by CargoFreshnessExplorer's later read. CargoFreshnessExplorer's claim cannot be attributed because OMP has a documented silent settings writer. `/tmp/mr-crabs-fix-config-audit.md` records the evidence. No root restoration or configuration change occurred.

### Supervisor status discriminator

The controlled run used the refreshed `target/release/mr-crabs` with no arguments. Before stop, hub and CUA agreed on the same pid `91546`. The native window was `Mr Crabs — shell`, window id `20054`. `ps` recorded process group and session `91546`.

`hub stop mr-crabs-stop-status-map` then returned `exited exit=1` after 20 seconds. The native window disappeared with the same pid. This proves that this hub status can result from managed termination of a successfully launched GUI process. It is not an application launch-time exit-1.




### Throughput checkpoint

- Blocking first steps. Rebuild the exact target binary in a fresh target directory. Observe a native window and map hub status against the same pid before considering a patch. Complete.
- Independent workstreams. Startup source mapping, regression history, runtime diagnostics, historical-pane recovery, configuration-scope review, and supervisor mapping produced separate evidence artifacts. Complete.
- Shared mutable state. The root alone writes `DOGFOOD-CRABS.md` and `.audit/fresh-release-exit-1.tsv`. Each delegate owns a separate `/tmp` report. No shared source writer exists.
- Smallest safe decomposition. The placement review found no in-repository script belongs here. The host harness owns the remaining status split.

### Freshness evidence limit

The isolated build differs by SHA-256 from the earlier release binary. That difference alone does not prove the earlier binary was stale. Its modification time follows the listed source files, and both binaries contain the WIP `+animation` error strings. The missing delegate artifact made its stale-binary conclusion unusable. The repeatable finding does not depend on that conclusion. Both the earlier artifact and the rebuilt target created a live native window.

### Fix plan and scope decision

The observation data shape is one launch record. It carries the binary SHA-256, the hub pid, the window-owner pid, whether a native window appeared, liveness duration, the control exit status, and managed-stop status. The matched-PID stop record has equal hub and window pids, live window success, and managed-stop `1`. The separate `--shell /usr/bin/true` control returned `0`.

After the controlled managed stop, hub reported `exited exit=1` for a process that had already opened a native window. The child wait encoding was not captured. The app's default launch passed. The `--shell /usr/bin/true` control exited `0`, but it does not prove a natural exit status for the default GUI. Changing `mr-crabs` would hide an unverified status interpretation.

No repository launch-verifier script is planned. `verification/tools` owns headless release gates. The status split belongs in the host harness where child and wrapper waits can be recorded separately. The placement review is `/tmp/mr-crabs-launch-verifier-placement.md`.

A host-harness correction is outside this repository and has not been authorized.

### Resume scope closure

The inherited fresh-release exit-1 lane remains closed. The live no-argument launch records at lines 100-121 found no launch-time status 1. The only repeatable `exit=1` came from `hub stop` after a live window. No source patch or repository launch verifier is justified. The resumed coordinator did not launch, stop, or drive Mr Crabs. `TODO-CRABS.md` was absent, so this note belongs in the existing repository evidence log.

The nearby OMP default-route drift is a separate host-configuration issue. It is not evidence about Mr Crabs and remains untouched under the current scope.

### Relay reproduction

At `2026-08-26T13:59:33+1200`, I independently reran the current `target/release/mr-crabs` after the cross-lane relay. Its SHA-256 was `026f86727dd8f268fc0d1855468775d7a32e6800f08f702bdef547bb3ac307c1`. Hub started `mr-crabs-relay-repro` with pid `85694`. The shell used `exec`, so that pid was the app pid.

CUA found the on-screen `Mr Crabs — shell` window `20126` for pid `85694`. Two snapshots found the same native window. Hub still reported the process ready after 2 minutes 43 seconds. Its log contained only `MR_CRABS_RELAY_READY`.

The current binary did not reproduce an immediate exit `1`. This is independent runtime evidence. It does not recover the missing w23:p3 child status or prove the origin of the historical report. No Mr Crabs source patch is justified.



