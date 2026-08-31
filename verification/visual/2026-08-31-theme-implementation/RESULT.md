# Lane C implementation: themes and window material

routing: cli-proxy/dddai.grok-4.6, fallback unverified

Status: PASS after overlay validation repair

throughput checkpoint: targeted settings overlay tests + `cargo test -p mr-crabs-config --offline parse_theme` + `cargo test -p mr-crabs-app --offline aliases_map` + `cargo check -p mr-crabs-app --offline`

Playbook: Feature. Principle leaves that changed choices: **find-not-invent** (config keys ADOPT-EXISTING; theme.rs BUILD-JUSTIFIED), **model-the-domain** (ThemeId + ResolvedChrome, not more workspace string matches), **laziness-protocol** (no extra theme names; GPUI Blurred/Opaque only, no radius field), **boundary-discipline** (policy injected, never detect() from theme).

---

## Find-not-invent

Class: Code reuse (config key) plus Pattern (theme resolve).

### Config keys: ADOPT-EXISTING

Stage 1 this repo. `grep SettingKey|parse_theme|background-opacity` over `crates/mr-crabs-config/src/lib.rs` and `crates/mr-crabs-app/src/settings.rs`. Owner is `SettingKey` plus `ConfigOverlay::set` / `EffectiveConfig`.

Findings. `SettingKey` already owns kebab flags, `ALL`, `from_flag`, `docs`, overlay merge, `display_value`. `parse_theme` was `auto|dark|light` at `lib.rs`. `background-opacity` already exists.

Rejected new config crate or parallel key enum. Same machinery takes `BackgroundBlur` and theme aliases.

Patch. Added `SettingKey::BackgroundBlur`, flag `background-blur`, default `0`. `parse_theme` now `auto|ink|paper|harbor|ember` with `dark` -> `ink`, `light` -> `paper`.

### Theme resolving: BUILD-JUSTIFIED

Stage 1. Workspace inlined `match settings.theme` at `crates/mr-crabs-app/src/ui/workspace.rs` (former 354-365). `crates/mr-crabs-app/src/palette.rs` is the command palette, not colors. `crates/mr-crabs-element/src/palette.rs` owns RGB constructors.

Stage 2. Design RESULT named `theme.rs` as the seam.

Stage 5. CLOSED by brief (no extra external research this land). Would have searched Ghostty `background-blur` vs GPUI `WindowBackgroundAppearance`.

Rejection of keeping the workspace match. That match would grow Harbor/Ember plus policy plus material. Wrong layer.

Boundary of `crates/mr-crabs-app/src/theme.rs`. Config owns parse/keys. Settings persist strings. Palette owns RGB. Theme maps those plus `WindowAppearance` plus injected `AccessibilityPolicy` into `ResolvedChrome`. Never calls `detect()`.

---

## Blast radius

What it does. Named themes, blur key, Reduce Transparency maps opacity to 1 and blur to 0. Workspace paints resolved palette and sets GPUI `WindowBackgroundAppearance`. CLI, runtime, and JSON file overlays all call `ConfigOverlay::set(SettingKey::Theme, ...)` so `"theme":"dark"` canonicalizes to `ink` and unknown names fail before commit.

The one safety fact. Illegal theme and blur values fail at the config boundary, and Reduce Transparency never leaves translucent chrome.

Proof, step 4.

- `parse_theme_accepts_named_themes_and_aliases` plus `dracula` reject.
- `background_blur_rejects_negatives_and_non_integers`.
- `file_overlay_canonicalizes_dark_to_ink`.
- `file_overlay_rejects_unknown_theme_atomically` (`SettingsError::Invalid`, generation unchanged).
- `file_overlay_rejects_invalid_background_blur_json` (negative, float, string).
- `reduce_transparency_forces_opaque_paper` (`from_flags(false, true)` -> Paper, opacity 1, Opaque).
- `named_harbor_ignores_light_desktop`.
- `cargo check -p mr-crabs-app --offline` succeeded.

Risks.

- GPUI `WindowBackgroundAppearance::Blurred` has no radius. User `background-blur` is stored and used as on/off. Radius is data only.

Cleared.

- DiagnoseCThemeValidation: PRE-EXISTING, not worsened by C. File JSON copied `theme: Option<String>` into `ConfigOverlay` and skipped `set`. Repair is `into_layers` calling `set(SettingKey::Theme)` before commit. `background_blur` stays serde `u16`.
- `AccessibilityPolicy` file untouched. Consumer only.
- No Metal. No `element.rs` trail edits.
- Existing `"theme":"dark"` JSON still loads as canonical `ink`.

---

## GPUI material

Pinned GPUI `03e5ad8a` `crates/gpui/src/platform.rs` `WindowOptions.window_background: WindowBackgroundAppearance` with `Opaque | Transparent | Blurred | MicaBackdrop | MicaAltBackdrop`. `Blurred` has no radius. `Window::set_background_appearance` exists.

Applied. Render maps `WindowMaterial::Opaque` -> `Opaque`, `Blurred { .. }` -> `Blurred`. Radius is not sent to GPUI because the enum has no field. Not faked in Metal. `shell.rs` left default `WindowOptions`; live updates use the window API.

---

## Tests

`CARGO_TARGET_DIR=~/.cargo-target`

- `cargo test -p mr-crabs-config --offline parse_theme` PASS
- `cargo test -p mr-crabs-config --offline background_blur` PASS
- `cargo test -p mr-crabs-config --offline theme_and_background` PASS
- `cargo test -p mr-crabs-element --offline configured_palette` PASS
- `cargo test -p mr-crabs-app --offline auto_light` PASS
- `cargo test -p mr-crabs-app --offline reduce_transparency` PASS
- `cargo test -p mr-crabs-app --offline named_harbor` PASS
- `cargo test -p mr-crabs-app --offline aliases_map` PASS
- `cargo test -p mr-crabs-app --offline opaque_user` PASS
- `cargo test -p mr-crabs-app --offline layered_precedence` PASS
- `cargo check -p mr-crabs-app --offline` PASS (existing dead_code warnings in element.rs)

---

## Files changed

- `crates/mr-crabs-config/src/lib.rs`
- `crates/mr-crabs-app/src/settings.rs`
- `crates/mr-crabs-app/src/lib.rs`
- `crates/mr-crabs-app/src/theme.rs` (new)
- `crates/mr-crabs-app/src/ui/workspace.rs`
- `crates/mr-crabs-element/src/palette.rs`
- `verification/visual/2026-08-31-theme-implementation/RESULT.md`

Not edited. `shell.rs` (no extra WindowOptions field needed; appearance is set on the live window). `accessibility_policy.rs`.
