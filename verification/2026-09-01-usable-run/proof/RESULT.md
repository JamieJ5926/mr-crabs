# Proof run 2026-09-01 usable-run

Agent: ProofRun (owner). No crate source edited. No git mutations.

## Build

Workspace: `/Users/jamie/.omp/wt/t5da83cd9f/m`

Verbatim final cargo line:

```
    Finished `release` profile [optimized] target(s) in 0.88s
```

(Preceding future-incompat note for `block v0.1.6` is a warning, not a failure.)

`CARGO_TARGET_DIR` resolved to `/Users/jamie/.cargo-target`. Binary used:

```
146133338 -rwxr-xr-x 1 jamie staff 12459296 Sep  1 21:19 /Users/jamie/.cargo-target/release/mr-crabs
mtime=2026-09-01 21:19:44
```

## Bundle refresh (rm-then-copy)

Quit old pids 45161 and 49978. `pgrep -fl mr-crabs` after quit: none (`NONE`).

Then:

```
rm -f ~/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
cp /Users/jamie/.cargo-target/release/mr-crabs ~/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
```

`ls -li` after copy (new inode, mtime 21:20):

```
146145613 -rwxr-xr-x 1 jamie staff 12459296 Sep  1 21:20 /Users/jamie/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
```

## Launch and liveness

Launched with `cua-driver launch_app {"bundle_id":"dev.jamie.mr-crabs"}` (not `open`).

`pgrep -fl mr-crabs` after launch:

```
39878 /Users/jamie/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
```

`list_windows` for pid 39878 (non-zero bounds, `is_on_screen: true`):

```
title: "Mr Crabs — shell"
window_id: 1546
bounds: {height: 949.0, width: 1512.0, x: 0.0, y: 33.0}
is_on_screen: true
on_current_space: true
```

Process did not die. No crash report captured.

## Process survival

Final `pgrep -fl mr-crabs` after every capture:

```
39878 /Users/jamie/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
```

Same pid 39878 from launch through 08. Window 1546 still on-screen with bounds 1512x949 after Cmd+W. **The process survived every step.**

## AX grid text

Every `.tree.txt` is chrome plus menus. There is no terminal grid text in AX. For `03-seq40`, the last printed number visible in the AX dump cannot be stated: **the AX dump exposes no grid text.**

This model cannot decode PNGs. Visual claims are `INCONCLUSIVE-NEEDS-EYES` for the root (image-capable) to settle from the named files.

## Checks

| Check | Verdict | Evidence |
|---|---|---|
| Release build | PASS | cargo final line `Finished \`release\` profile [optimized] target(s) in 0.88s` |
| Quit old process | PASS | `pgrep` after kill printed `NONE` |
| rm-then-copy | PASS | inode `146145613`, mtime `Sep 1 21:20` |
| Launch liveness pid | PASS | pid `39878` |
| Launch liveness window | PASS | window 1546, 1512x949, `is_on_screen: true` |
| 01 launch idle, no input (blank-surface) | INCONCLUSIVE-NEEDS-EYES | `01-launch-idle.png` + `01-launch-idle.tree.txt` |
| 02 after one Return | INCONCLUSIVE-NEEDS-EYES | `02-after-enter.png` + `02-after-enter.tree.txt`. First Return without window_id was `ambiguous_window_target`; retried with window_id 1546. |
| 03 `seq 1 40` (dock hiding rows) | INCONCLUSIVE-NEEDS-EYES | `03-seq40.png` + `03-seq40.tree.txt`. AX dump exposes no grid text; last printed number not available from AX. Background `type_text` failed (`delivery_failed` -> foreground); seq typed with `delivery_mode: foreground`. |
| 04 vim alt-screen full height | INCONCLUSIVE-NEEDS-EYES | `04-vim-alt-screen.png` + `04-vim-alt-screen.tree.txt` |
| 05 after `:q!` reflow | INCONCLUSIVE-NEEDS-EYES | `05-after-vim-quit.png` + `05-after-vim-quit.tree.txt` |
| 06 Cmd+D split, new pane prompt | INCONCLUSIVE-NEEDS-EYES | `06-split-right.png` + `06-split-right.tree.txt`. AX click on New Split Right refused (`element_outside_target_window`); used foreground Cmd+D. |
| 07 Cmd+T new tab | INCONCLUSIVE-NEEDS-EYES | `07-new-tab.png` + `07-new-tab.tree.txt` |
| 08 Cmd+W closes tab, window stays | INCONCLUSIVE-NEEDS-EYES for tab chrome; window survival PASS | `08-cmd-w-closed-tab.png` + `08-cmd-w-closed-tab.tree.txt`. After Cmd+W, pid 39878 still running; window 1546 still 1512x949 on-screen. |
| Process survival through all steps | PASS | pid 39878 throughout; final pgrep same |

## Deviations

- Input to the GPUI terminal needed `delivery_mode: "foreground"` after background type_text reported `delivery_failed`.
- Menu-bar AX click for New Split Right is not in the window AX surface.
- `animated_fetch` historical flake was not reproduced (out of scope).

## Unresolved risks

Root must read the eight PNGs. I did not visually confirm prompt, seq 40 last line, vim full height, split prompt, or tab chrome.
