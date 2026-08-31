# Lane C design: themes and window material

routing: cli-proxy/dddai.grok-4.6, fallback unverified

Status: PASS (design artifact only). No source edits. No GUI. No git mutation.

throughput checkpoint: n/a, read-only investigation

Playbook: Investigation. Principle leaves that changed choices: **model-the-domain** (named theme enum plus resolved palette, not more string matches), **experience-first** (four palettes, not eighteen), **laziness-protocol** (keep `theme` and `background-opacity` keys; add one blur key), **never-block-on-the-human** (pick the four names here). **how** child spawn skipped: this seat cannot spawn children.

---

## Overview

Config today stores `theme` as `auto | dark | light` (`parse_theme` at `crates/mr-crabs-config/src/lib.rs:836-841`). Workspace maps that string onto `TerminalPalette::light` or `::dark` and, for `auto`, onto `window.appearance()` (`crates/mr-crabs-app/src/ui/workspace.rs:325-336`). Opacity is a float on the palette (`background_opacity`, default `1.0`). Windows open with default `WindowOptions` and no background material (`crates/mr-crabs-app/src/ui/shell.rs:266-274`). Lane E already owns `AccessibilityPolicy::allows_transparency()` (`crates/mr-crabs-app/src/accessibility_policy.rs:36-38`). This design keeps that boundary and names four palettes plus one resolved window-material path through GPUI, never a Metal pass.

## Key concepts

**ThemeId.** Closed set: `Auto`, `Ink`, `Paper`, `Harbor`, `Ember`. User strings stay kebab-case. `auto` is not a palette. It picks `Ink` or `Paper` from `WindowAppearance`.

**ThemeSpec.** Frozen RGB roles: foreground, background, cursor, selection, plus a 16-color ANSI table for the named theme. `TerminalPalette` grows to carry the ANSI table so `named_rgb_with_palette` stops using the global xterm table for 0-15 when a named theme is active.

**ResolvedChrome.** Runtime only. `{ palette: TerminalPalette, window_material: Opaque | Blurred { radius } }`. Built from user opacity, user blur, and `policy.allows_transparency()`. Never persisted.

**User translucency.** Keep `background-opacity` in `0.0..=1.0`. Add `background-blur` as a non-negative integer intensity, default `0` (off). Ghostty-shaped flag name, GPUI material behind it.

## Data shape

```text
ThemeId = Auto | Ink | Paper | Harbor | Ember

ThemeSpec {
  id: ThemeId  // never Auto
  appearance: Dark | Light
  foreground, background, cursor, selection: [u8; 3]
  ansi: [[u8; 3]; 16]
}

UserVisual {
  theme: ThemeId            // config string
  background_opacity: f32   // 0.0..=1.0, default 1.0
  background_blur: u16      // 0 = off, default 0
}

ResolvedChrome {
  palette: TerminalPalette  // opacity forced to 1.0 when !allows_transparency
  material: Opaque | Blurred { radius: u16 }
}
```

Resolve once at the app boundary:

1. Map `ThemeId` + `WindowAppearance` to `ThemeSpec`. `auto` + Light/VibrantLight -> Paper. `auto` + Dark/VibrantDark -> Ink. Named ids ignore desktop appearance for colors.
2. `effective_opacity = policy.allows_transparency() ? user.background_opacity : 1.0`.
3. `effective_blur = policy.allows_transparency() && effective_opacity < 1.0 ? user.background_blur : 0`.
4. Build `TerminalPalette` from `ThemeSpec` with `effective_opacity`.
5. `material = Opaque` if `effective_blur == 0`, else `Blurred { radius: effective_blur }`.

Reduce Transparency never reads NSWorkspace here. Inject `AccessibilityPolicy` (tests use `from_flags`). Paint and config crates do not call `detect()`.

## Palettes (four, art-directed)

Not eighteen clones. Roles are eight-ish (fg, bg, cursor, selection, plus ANSI). Values below are the intended first land. Implementer may nudge after visual capture, not add names.

**Ink** (dark, `auto` dark). Near-black paper, cool gray text. bg `#0D0D0D`, fg `#E5E5E5`, cursor `#E5E5E5`, selection `#5278C8`. Today's `TerminalPalette::dark`. Default `auto` dark path stays familiar.

**Paper** (light, `auto` light). Warm paper, not pure white. bg `#F5F5F5`, fg `#202020`, cursor `#202020`, selection `#5278C8`. Today's `TerminalPalette::light`.

**Harbor** (dark, named). Cooler, slightly lifted bg so translucency reads against a wallpaper. bg `#12161C`, fg `#D7E0EA`, cursor `#7EB6D6`, selection `#3D6A8A`. ANSI shifted toward teal/steel, not neon.

**Ember** (dark, named). Warm ink, one accent. bg `#1A1410`, fg `#E8DCC8`, cursor `#E07A3D`, selection `#8A4A28`. ANSI reds/yellows lifted, blues muted. The fourth palette earns its slot as the "evening" look. A fourth light theme does not. Light desktop is Paper.

ANSI 0-15 for Ink/Paper stay the current `ANSI_PALETTE` in `palette.rs`. Harbor and Ember get their own 16. Do not ship extra named schemes.

## Reduce Transparency mapping

Consumer is C. Owner of the flag is E.

| User | Policy | Effective opacity | Effective blur | Cell bg alpha |
| --- | --- | --- | --- | --- |
| opacity 1, blur 0 | any | 1 | 0 | 1 |
| opacity 0.85, blur 20 | allows_transparency | 0.85 | 20 | 0.85 |
| opacity 0.85, blur 20 | !allows_transparency | 1 | 0 | 1 |
| opacity 0.85, blur 0 | allows_transparency | 0.85 | 0 | 0.85 (clear, no blur) |

Selection overlay stays at 0.3 alpha on the selection RGB. It is a highlight, not window material. Reduce Transparency does not force selection opaque. Trail leftover is B's problem, not this gate.

Window material uses GPUI `WindowOptions` background appearance / blur that the crate already exposes. Later implementer looks up the current GPUI field names at land time. Do not add `crates/**/*.metal` or an app compositor. `verification/research/gpu-terminal-architecture.md:8-46` still holds.

## Implementation seam (later C write, not this unit)

**Config (`mr-crabs-config`, C).**

- Replace `parse_theme` allow-list with `auto|ink|paper|harbor|ember`. Keep `dark` and `light` as aliases: `dark` -> `ink`, `light` -> `paper`. Persist the canonical name on next save so files converge.
- Add `SettingKey::BackgroundBlur`, flag `background-blur`, parse non-negative integer. Default `0`. Bump `SettingKey::ALL` length.
- Docs on `Theme` mention the four names and the aliases.

**Settings (`crates/mr-crabs-app/src/settings.rs`, C).**

- `AppSettings.theme` stays `String` or becomes `ThemeId` if serde is cheap. Prefer `ThemeId` with serde string if it does not break unknown-key preservation.
- New field `background_blur: u16`.
- Overlay merge and `get`/`set` for the new key.

**Resolve helper (new small fn in app, C).** Suggested `crates/mr-crabs-app/src/theme.rs` so workspace does not grow another string match.

```text
fn resolve_chrome(
  theme: &str,
  background_opacity: f32,
  background_blur: u16,
  appearance: WindowAppearance,
  policy: AccessibilityPolicy,
) -> ResolvedChrome
```

Workspace today inlines the match at `workspace.rs:325-336`. Replace that block with `resolve_chrome(...)`. Pass `policy` from the model snapshot E already (or will) store. Do not call `AccessibilityPolicy::detect()` from workspace or palette.

**Palette (`crates/mr-crabs-element/src/palette.rs`).** K owns `palette.rs` per standing file split. C requests the type change; K or a later C+K unit applies it.

- Add `Ink`/`Paper` constructors (rename of `dark`/`light` or keep those as aliases).
- Add `harbor` / `ember` constructors.
- Optional `ansi: [[u8;3];16]` on `TerminalPalette`. If K refuses the field, Harbor/Ember still change fg/bg/cursor/selection only on first land. Full ANSI is the better domain model.

**Window material (`crates/mr-crabs-app/src/ui/shell.rs`).** `open_native_window` sets `WindowOptions` from resolved material. If GPUI applies material per window at open only, live opacity/blur changes need a documented GPUI update API or a window recreate. Record the chosen GPUI call in the implementer RESULT. Still no Metal.

**Not C.** `element.rs` paint_trail (B). `paint_text_reveal` (I). `palette.rs` glyph mapping beyond the constructors (K). `accessibility_policy.rs` (E). `mr-crabs-effects/` (A/B/I).

### Config migration

- Missing `theme` -> `auto` (existing default).
- `theme: "dark"` reads as Ink. Next write emits `ink`.
- `theme: "light"` reads as Paper. Next write emits `paper`.
- Unknown theme string is a parse error, same as today (`unknown` already fails tests).
- Missing `background-blur` -> `0`. No file rewrite required.
- `background-opacity` unchanged.

### Tests (later unit)

Headless, no window.

1. `parse_theme` accepts `auto|ink|paper|harbor|ember|dark|light`, rejects `dracula`.
2. Alias: overlay set `dark` stores canonical `ink` (or stores `dark` and resolve maps it; prefer canonical on set).
3. `resolve_chrome("auto", 0.8, 20, Light, from_flags(false,false))` -> Paper, opacity 0.8, Blurred 20.
4. Same with `from_flags(false, true)` -> Paper, opacity 1.0, Opaque.
5. `resolve_chrome("harbor", 0.8, 20, Light, allows)` -> Harbor colors even on light desktop.
6. `background-blur` parse rejects negatives and non-integers.
7. Existing settings tests that seed `"theme":"dark"` still load.

No GPUI window fixture in the first land unless one already exists.

## Visual matrix plan (later capture, not this unit)

Four stills prove static composition. Desktop appearance is OS light/dark, not the theme name. Capture against one Mr Crabs window (standing order 11).

Fixed user config for the matrix: `theme = auto`, `background-opacity = 0.82`, `background-blur = 20`. Wallpaper must be high-contrast so blur vs opaque is visible (photograph or color blocks behind the window).

| Shot | Desktop | Reduce Transparency | Expected |
| --- | --- | --- | --- |
| A | Light | Off | Paper palette, desktop shows through, blur on |
| B | Light | On | Paper palette, fully opaque, no blur |
| C | Dark | Off | Ink palette, desktop shows through, blur on |
| D | Dark | On | Ink palette, fully opaque, no blur |

Named themes extra (optional, after the four): Harbor and Ember on a dark desktop, Reduce Transparency off, same opacity/blur. Not required for Gate 4 minimum.

Capture method. Single-window pid. `cua-driver` snapshots. No animation claim. A still is valid here because this is static chrome. Inject policy via test override if toggling System Settings is unsafe in the capture machine; the four images must still show opaque vs translucent pixels, not just a log line.

Pass criteria. A vs B (and C vs D) differ in background alpha or wallpaper bleed. A vs C differ in Ink vs Paper RGB. B and D show no wallpaper texture in the content rect.

## How it works (current vs target)

Today. Settings string -> workspace match -> `TerminalPalette::{light,dark}(opacity)` -> element paints cell bg with that alpha. Window is opaque platform chrome. Reduce Transparency is unused by C.

Target. Settings `ThemeId` + opacity + blur + `WindowAppearance` + `AccessibilityPolicy` -> `ResolvedChrome` -> palette to the element, material to `WindowOptions`. Same paint path. Different inputs.

## Where things live

| Path | Role |
| --- | --- |
| `crates/mr-crabs-config/src/lib.rs` | keys, parse, defaults |
| `crates/mr-crabs-app/src/settings.rs` | persisted settings |
| `crates/mr-crabs-app/src/theme.rs` | new resolve helper |
| `crates/mr-crabs-app/src/ui/workspace.rs` | call resolve, pass palette |
| `crates/mr-crabs-app/src/ui/shell.rs` | window material |
| `crates/mr-crabs-app/src/accessibility_policy.rs` | E, consume only |
| `crates/mr-crabs-element/src/palette.rs` | K, ThemeSpec constructors |

## Gotchas

- `palette.rs` is K's file. C must not sneak-edit it in a later land without the ownership handshake.
- `dark`/`light` aliases exist because current files and tests use them.
- Opacity < 1 with blur 0 is legal. Clear glass without blur.
- Opacity 1 with blur > 0 is a no-op. Blur only when the background is actually translucent.
- `WindowAppearance::Vibrant*` already maps in workspace. Keep that in resolve.
- Do not put `AccessibilityPolicy` in `mr-crabs-config` or `mr-crabs-element`.

## Rejected options

| Option | Why not |
| --- | --- |
| Eighteen shipped themes | Standing order 8. Feature-count race. |
| App-owned Metal blur | Two critics plus gpu-terminal-architecture. GPUI material only. |
| Per-lane NSWorkspace reads | E owns detection. |
| Separate `theme-dark` / `theme-light` keys | One `theme` key already exists. `auto` plus four names is enough. |
| Fourth light palette | Paper covers light desktop. Harbor/Ember are dark character. |
| User CSS / infinite custom themes in v1 | Out of scope. Named closed set. |
| Forcing selection and trail opaque under Reduce Transparency | Wrong layer. Window material only. |
| Recreating `TerminalPalette::dark` under a new name without aliases | Breaks call sites for no gain. Keep `dark`/`light` constructors as Ink/Paper aliases if K prefers. |

## Follow-ups (out of SCOPE)

- Live window-material update API if GPUI cannot change blur without recreate.
- ANSI tables for Harbor/Ember if first land ships fg/bg/cursor/selection only.
- Command-palette theme picker (K owns `palette.rs` UI naming collision; that file is color mapping, not the command palette).

## Source change check

This unit wrote only `verification/visual/2026-08-31-theme-design/RESULT.md`.
