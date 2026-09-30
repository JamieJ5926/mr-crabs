# Terminal matrix captures index

Two capture kinds live in this directory.

Text frames from the matrix harness (landed): `crabs.txt`, `ghostty.txt`,
`vm.txt`, plus the generated `gap-table.md`. All four gap rows PASS on all
three columns in the latest harness run (exit 0).

Pixel pairs from the GUI drive lanes (pending): one
`shot-<gap>-before.png` / `shot-<gap>-after.png` pair per gap, per
`skill://control-mr-crabs`. None have landed yet. Filenames below are the
contract, not links. Do not convert them to image links until the files
exist.

| Gap | Before | After | Status |
| --- | ------ | ----- | ------ |
| g8-first-exec | `shot-g8-first-exec-before.png` | `shot-g8-first-exec-after.png` | pending GUI drive; text probes PASS (blank-marker, first-exec-nonblank) |
| sigwinch-resize | `shot-sigwinch-resize-before.png` | `shot-sigwinch-resize-after.png` | pending GUI drive; text probe PASS (resize-80x24) |
| term-terminfo | `shot-term-terminfo-before.png` | `shot-term-terminfo-after.png` | pending GUI drive; text probe records TERM per frame |
| osc52-clipboard | `shot-osc52-clipboard-before.png` | `shot-osc52-clipboard-after.png` | pending GUI drive; text probe PASS (osc52-bounded<=2s) |
| osc8-hyperlink | `shot-osc8-hyperlink-before.png` | `shot-osc8-hyperlink-after.png` | pending GUI drive; text probe has OSC8-LINK section |
| sgr-edge | `shot-sgr-edge-before.png` | `shot-sgr-edge-after.png` | pending GUI drive; text probe has STYLES section |
