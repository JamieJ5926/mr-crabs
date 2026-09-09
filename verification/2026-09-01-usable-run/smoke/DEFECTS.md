# Smoke defects — 2026-09-01

Liveness (verbatim):

```
3647 /Users/jamie/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs
```

`cua-driver list_windows '{"pid":3647}'` (on-screen window):

```
{
  "app_name": "Mr Crabs",
  "bounds": { "height": 949.0, "width": 1512.0, "x": 0.0, "y": 33.0 },
  "is_on_screen": true,
  "pid": 3647,
  "title": "Mr Crabs — shell",
  "window_id": 767
}
```

Pid 3647 stayed alive through all 18 paths. No crash / no relaunch.

GPUI terminal buffer is not exposed as AX text. Paths whose truth is only in pixels are `INCONCLUSIVE-NEEDS-EYES`. PNG byte-size changes are noted but are **not** used as PASS.

| path | verdict | screenshot | tree | notes / root-cause pointer |
|---|---|---|---|---|
| 01 prompt-idle | PASS | `01-prompt-idle.png` | `01-prompt-idle.tree.txt` | AXWindow `"Mr Crabs — shell"`; bounds 1512×949; pid 3647. Prompt glyphs themselves are pixels → parent eyes on PNG. |
| 02 echo-hello | INCONCLUSIVE-NEEDS-EYES | `02-echo-hello.png` | `02-echo-hello.tree.txt` | Typed `echo hello`+Return (`delivery_mode:foreground`). AX has no buffer text (`hello` not in tree). |
| 03 ls-la | INCONCLUSIVE-NEEDS-EYES | `03-ls-la.png` | `03-ls-la.tree.txt` | Typed `ls -la`+Return. AX unchanged aside from chrome. |
| 04 cmd-t-new-tab | INCONCLUSIVE-NEEDS-EYES | `04-cmd-t-new-tab.png` | `04-cmd-t-new-tab.tree.txt` | `hotkey cmd+t`. AX still one `AXWindow`; no tab count. Menu item `New Tab` exists (chrome). keymap: `crates/mr-crabs-app/src/keymap.rs` CloseTab/NewTab bindings. |
| 05 cmd-d-split-right | INCONCLUSIVE-NEEDS-EYES | `05-cmd-d-split-right.png` | `05-cmd-d-split-right.tree.txt` | `hotkey cmd+d`. AX no split nodes. Menu `New Split Right` present. |
| 06 cmd-shift-d-split-down | INCONCLUSIVE-NEEDS-EYES | `06-cmd-shift-d-split-down.png` | `06-cmd-shift-d-split-down.tree.txt` | `hotkey cmd+shift+d`. AX no split nodes. Menu `New Split Down` present. |
| 07 ctrl-cmd-arrow-pane-focus | INCONCLUSIVE-NEEDS-EYES | `07-ctrl-cmd-arrow-pane-focus.png` | `07-ctrl-cmd-arrow-pane-focus.tree.txt` | `hotkey ctrl+cmd+right`. PNG size identical to 06; AX identical. Menu `Move Focus Right` present. |
| 08 cmd-bracket-tab-cycle | INCONCLUSIVE-NEEDS-EYES | `08-cmd-bracket-tab-cycle.png` | `08-cmd-bracket-tab-cycle.tree.txt` | `hotkey cmd+]`. AX still one window title. Menu `Next Tab` present. |
| 09 drag-select-then-cmd-c | INCONCLUSIVE-NEEDS-EYES | `09-drag-select-then-cmd-c.png` | `09-drag-select-then-cmd-c.tree.txt` | `drag` then `cmd+c`. `clipboard_read` reported types `NSStringPboardType`, `public.utf8-plain-text` with `text: null` (redacted). Cannot confirm copied payload. |
| 10 cmd-v-paste | INCONCLUSIVE-NEEDS-EYES | `10-cmd-v-paste.png` | `10-cmd-v-paste.tree.txt` | `hotkey cmd+v`. No paste string in AX. |
| 11 multiline-paste | INCONCLUSIVE-NEEDS-EYES | `11-multiline-paste.png` | `11-multiline-paste.tree.txt` | `clipboard_write` of `echo line1\necho line2` (`written_type: text`) then `cmd+v`. No multiline text in AX. |
| 12 wheel-scroll-up | INCONCLUSIVE-NEEDS-EYES | `12-wheel-scroll-up.png` | `12-wheel-scroll-up.tree.txt` | `scroll direction=up amount=8`. PNG size same as 11; AX same. |
| 13 wheel-scroll-back | INCONCLUSIVE-NEEDS-EYES | `13-wheel-scroll-back.png` | `13-wheel-scroll-back.tree.txt` | `scroll direction=down`. AX same. |
| 14 cmd-shift-r-reload | INCONCLUSIVE-NEEDS-EYES | `14-cmd-shift-r-reload.png` | `14-cmd-shift-r-reload.tree.txt` | `hotkey cmd+shift+r`. PNG size same as 13. Menu `Reload Configuration` present. |
| 15 window-resize | INCONCLUSIVE-NEEDS-EYES | `15-window-resize.png` | `15-window-resize.tree.txt` | First zoom click refused (`snapshot_id_required`). Later `element_token` click: `effect: unverifiable`. Window 767 bounds remained `{height:949,width:1512,x:0,y:33}`. |
| 16 cmd-shift-p-palette | INCONCLUSIVE-NEEDS-EYES | `16-cmd-shift-p-palette.png` | `16-cmd-shift-p-palette.tree.txt` | `hotkey cmd+shift+p`. `Command Palette` is always in the View menu AX; no extra palette window/role appeared. |
| 17 ctrl-backtick-quick-terminal | INCONCLUSIVE-NEEDS-EYES | `17-ctrl-backtick-quick-terminal.png` | `17-ctrl-backtick-quick-terminal.tree.txt` | `hotkey ctrl+\``. Tree grew 10402→10503 chars; still one AXWindow. Menu `Quick Terminal` present. |
| 18 tui-render | INCONCLUSIVE-NEEDS-EYES | `18-tui-render.png` | `18-tui-render.tree.txt` | `which htop` empty; `which vim` → `/usr/bin/vim`. Typed `htop` then `vim`+Return. No TUI roles in AX. |

No DEFECT (crash) recorded. Visual PASS/FAIL is for parent image review of the 18 PNGs.
