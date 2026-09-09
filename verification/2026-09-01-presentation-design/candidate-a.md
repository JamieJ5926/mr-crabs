# Candidate A: theme-owned presentation

## 1. Data shape first

The theme must resolve to one typed value before settings, rendering, or startup code makes a choice.

```rust
// mr-crabs-config: shared setting domain
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptPresentation {
    Inline,
    Dock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchArrangement {
    Beside,
    Below,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupArt {
    None,
    BuiltIn(Cow<'static, str>),
    File(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemePresentationDefaults {
    pub prompt: PromptPresentation,
    pub startup_animation: StartupAnimation,
    pub art: StartupArt,
    pub fetch_arrangement: FetchArrangement,
}

// mr-crabs-app::theme: one row per named theme
pub struct ThemeDefinition {
    pub id: ThemeId,
    pub aliases: &'static [&'static str],
    pub palette: PaletteSource,
    pub presentation: ThemePresentationDefaults,
}

pub enum PaletteSource {
    Fixed(fn(f32) -> TerminalPalette),
    System {
        light: ThemeId,
        dark: ThemeId,
    },
}

pub struct ResolvedTheme {
    pub requested: ThemeId,
    pub palette_theme: ThemeId,
    pub palette: TerminalPalette,
    pub material: WindowMaterial,
    pub presentation_defaults: ThemePresentationDefaults,
}
```

`StartupAnimation` remains the existing `None | Rustfetch | Molt` enum. The new types replace loose strings after parsing. `StartupArt::BuiltIn` borrows static IDs in theme rows and owns IDs parsed from settings, so the static table needs no allocation. `StartupArt` is one discriminated value, not a path plus a `use_builtin` boolean. `PromptPresentation` is one enum, not `dock_enabled` plus an inline flag. Those choices prevent invalid combinations and keep the render path to one equality check.

`ThemeDefinition` lives in a static `THEMES` table. Parsing searches `id` and `aliases`; palette construction calls the stored function pointer. Adding a theme adds one data row. It does not add another arm to theme parsing, palette resolution, prompt selection, startup selection, and art selection.

`auto` is a real table row. Its `PaletteSource::System` follows GPUI appearance, while its presentation defaults belong to `auto` itself. A macOS light-to-dark appearance change may swap Paper and Ink colors, but it must not change a live prompt from inline to dock. `ResolvedTheme` records both the requested profile and the concrete palette profile so that distinction is visible in tests and diagnostics.

The current `resolve_chrome` should become `resolve_theme` and return `ResolvedTheme`. This extends the existing result rather than adding a parallel `resolve_presentation` call. A parallel call would create two theme lookups that could resolve aliases or `auto` differently. Window material remains global because opacity, blur, and accessibility policy are not named-theme properties today. `InputDockTokens::for_palette` also stays as it is. This candidate changes presentation ownership, not dock color ownership.

Initial profile rows should preserve current behavior wherever the request does not require a change:

| Theme | Prompt | Startup animation | Art | Fetch arrangement |
| --- | --- | --- | --- | --- |
| auto | dock | molt | `builtin:apple` | below |
| ink | dock | molt | `builtin:apple` | below |
| paper | dock | molt | `builtin:apple` | below |
| harbor | dock | molt | `builtin:apple` | below |
| ember | dock | molt | `builtin:apple` | below |

The rows deliberately start alike. The ownership change should not smuggle in five unrelated product decisions. Themes can diverge later by editing data, with no new control flow.

## 2. Theme defaults and explicit overrides

A theme supplies defaults. It never writes over an explicit presentation setting.

The existing chain remains `defaults < file < CLI < runtime`, but theme selection is resolved before the chain is materialized:

1. Select the final theme name with `runtime.theme.or(cli.theme).or(file.theme).unwrap_or("auto")`.
2. Look up that row in `THEMES` and obtain its `ThemePresentationDefaults`.
3. Seed the four presentation fields in the base `EffectiveConfig` from that row.
4. Apply the complete file overlay.
5. Apply the complete CLI overlay.
6. Apply the complete runtime overlay.

This is intentionally a two-stage calculation. The final theme can come from any layer, but its presentation values are still defaults. Every explicit presentation value from every layer is replayed above those defaults.

The config crate should enforce this through one resolution entry point rather than asking callers to remember two calls:

```rust
pub fn resolve(
    file: &ConfigOverlay,
    cli: &ConfigOverlay,
    runtime: &ConfigOverlay,
    theme_defaults: impl FnOnce(&str) -> ThemePresentationDefaults,
) -> EffectiveConfig
```

`EffectiveConfig::resolve` selects the winning theme name, calls the injected app-owned lookup, seeds the base, and then applies the three overlays in order. The callback preserves the current crate dependency direction. `mr-crabs-config` owns setting types and precedence; `mr-crabs-app::theme` owns the named theme table and palette constructors.

Concrete conflict answer: if Ember says `dock` and the user sets `prompt_presentation = "inline"` in the file, the effective value is `Inline`. It is still `Inline` if Ember itself was selected by a CLI or runtime theme override. A CLI `--prompt-presentation=dock` would then beat the file and return to `Dock`; a runtime `inline` would beat both. The theme never silently wins because its values enter below the file layer.

The four setting names are:

| CLI and `+show-config` key | JSON field | Values |
| --- | --- | --- |
| `prompt-presentation` | `prompt_presentation` | `dock`, `inline` |
| `startup-animation` | `startup_animation` | existing `none`, `rustfetch`, `molt` |
| `startup-art` | `startup_art` | `none`, `builtin:<id>`, `file:<path>` |
| `fetch-arrangement` | `fetch_arrangement` | `beside`, `below` |

`startup-fetch`, `startup-fetch-command`, and `fetch-gif-path` remain supported. `startup-fetch = false` remains the master opt-out for running startup fetch content. Startup settings affect new windows. Prompt presentation and theme colors apply on the next render after Cmd+Shift+R, matching the current reload split at `model/app_model.rs:1495-1517` and `ui/workspace.rs:339-373`.

## 3. Art registry

Art is data in a registry, not a match arm per logo.

```rust
pub struct ArtAsset {
    pub id: &'static str,
    pub text: &'static str,
}

pub static BUILTIN_ART: &[ArtAsset] = &[
    ArtAsset {
        id: "apple",
        text: include_str!("../assets/art/apple.txt"),
    },
];

pub struct LoadedArt {
    pub text: Cow<'static, str>,
    pub width_cells: u16,
    pub height: u16,
}
```

`startup_art.rs` owns `ArtAsset`, `BUILTIN_ART`, lookup, custom-file loading, and cell measurement. Built-in lookup is a table search by ID. `Cow<'static, str>` keeps embedded art borrowed and owns only custom file content. Width uses the existing visible-cell-width rules from `animated_fetch.rs`; that helper should move into the art module rather than be copied.

The Apple logo lives at `crates/mr-crabs-app/assets/art/apple.txt` and is embedded with `include_str!`. Embedded art is always present, needs no packaging rule, and cannot change independently of the binary. Its cost is that changing or adding built-in art requires a rebuild. A user selects `file:<path>`; relative paths resolve from the application launch directory, matching other process-relative paths in the current flat overlay model. The loader reads UTF-8 at startup, caps it at 64 KiB like rustfetch capture, normalizes CRLF, and measures it once. File art can change without rebuilding, but it can be missing or invalid at the next window open. A load failure should report the setting error and render fetch info without art, not silently substitute Apple for the user's explicit choice.

The built-in helper receives the effective art selection and arrangement through reserved child environment entries placed in the pending `PtySpawnConfig.env`. This avoids rewriting the user's `startup_fetch_command` or appending flags to a custom command. The default `+animated-fetch` helper reads those entries. A custom startup command remains opaque and may choose to honor the same entries; Mr Crabs must not reinterpret arbitrary command output.

## 4. How Molt composes with art

Write the composed art and fetch text into the terminal grid before Molt begins. Do not add an app-owned Metal path or a second text renderer.

The flow is:

1. `animated_fetch.rs` captures rustfetch, keeps the already-parsed `line.info` data, replaces the captured logo with the selected `LoadedArt`, and composes a `PresentationLayout`.
2. `FetchArrangement::Below` emits the art block first, a blank row, then the information rows aligned under the art's left edge. `Beside` emits a two-column block with a two-cell gap. The composed block, not the original rustfetch line width, drives centering and fit checks.
3. In `Rustfetch` mode, the existing frame sequence animates art cells and leaves info cells unchanged, then retains the final composed block. In `Molt` mode, the helper emits one static composed block immediately. Existing non-TTY, parse-failure, empty-capture, and does-not-fit paths remain, but their static success output includes info rather than discarding it.
4. `StartupPresentation` gains a `MoltWaiting` state. The window is fully masked while waiting. The first published pane frame transitions it to `MoltActive { started_ms }`. This ensures the growing hole reveals actual art instead of half a second of empty terminal caused by the default startup command's sleep.
5. `model/window.rs` continues to own elapsed time, sine-out geometry, glow alpha, completion, and the 16 ms cadence. `ui/shell.rs` continues to schedule those frames. `ui/workspace.rs` first paints the ordinary GPUI terminal element, then paints the four black GPUI rectangles and four cyan edge strips over it. The only visual change to Molt is what exists underneath the mask and when the clock starts.

This reuses the existing renderer boundary. The terminal owns text, ANSI style, Unicode width, selection, accessibility, and resize. GPUI owns the animated mask. Writing the block to the grid also means the final Apple art and system information remain normal terminal content after the 900 ms mask finishes. No duplicate text paint implementation can drift from terminal metrics.

The existing `FetchLine { logo, info }` split at `animated_fetch.rs:11-21` already contains the required data. The current loss happens in `logo_only_bytes`, `frame_bytes`, `positioned_frame`, `dimmed_frame`, and `centered_animation_chunks` at `animated_fetch.rs:23-320`. Replace their logo-only output with one composed layout. Do not add an `if arrangement == below` branch to each renderer. `PresentationLayout::compose(art, info, arrangement)` makes the one arrangement decision and gives every renderer the same positioned lines plus art spans.

## 5. Prompt presentation

Inline mode means the terminal grid remains visible and authoritative. Dock mode keeps the current semantic projection and input routing.

`ui/workspace.rs` reads the already-resolved `PromptPresentation` from `AppSettings`. `ResolvedTheme.presentation_defaults` records the profile source, but render code must not merge it again after config precedence has run. It gates two things with the same enum value:

- dock reserve before `SurfaceGeometry::from_viewport_with_dock_reserve`
- focused dock composition before the mask, separator, overlay, footer, and dock mouse routes are built

Inline mode passes `false` for reserve and does not construct a `FocusedDockRender`. It does not disable OSC 133 parsing, remove `InputDockSnapshot`, mutate the command string, or create another input path. The shell's own prompt and input row stay in the grid, so keyboard, IME, paste, mouse, and semantic state keep using the current terminal path. Switching back to Dock on reload uses the snapshot machinery already retained by `PaneModel`.

The gate belongs in `workspace.rs`, not in `PaneModel::should_reserve_dock_chrome`. The pane knows whether semantic dock chrome is eligible; the presentation setting decides whether this window wants that eligible chrome. This keeps theme policy out of the terminal model and avoids threading a second synchronized boolean through every pane.

## 6. Ordered change list and diff estimates

Estimates count changed or added lines, including focused tests. They are planning ranges, not measured diffs.

1. `crates/mr-crabs-app/assets/art/apple.txt`, new, 15 to 30 lines. Store the plain UTF-8 Apple asset consumed by `include_str!`.
2. `crates/mr-crabs-config/src/lib.rs`, 180 to 240 lines. Add `PromptPresentation`, `FetchArrangement`, `StartupArt`, and `ThemePresentationDefaults` before settings plumbing. Add constants or typed defaults. Extend `SettingKey` variant, `ALL`, `flag`, `from_flag`, and docs at the current `lib.rs:157-302` sites for the three new keys. Extend `ConfigOverlay` field, merge, apply, and set at `lib.rs:337-581`. Extend `EffectiveConfig` field, default seeding, callback-based resolve, display, and accessors at `lib.rs:594-710`. Add parser and precedence tests, especially runtime theme plus file inline.
3. `crates/mr-crabs-app/src/theme.rs`, 100 to 150 lines. Move from per-operation theme matches at `theme.rs:13-91` to the static `THEMES` definitions. Replace `resolve_chrome` with `resolve_theme`, retain accessibility-driven material resolution, and test requested `auto` versus concrete palette theme plus the presentation table.
4. `crates/mr-crabs-app/src/settings.rs`, 110 to 160 lines. Add serde fields and defaults near `settings.rs:120-208`; extend `from_effective`, `effective_config`, and typed accessors at `settings.rs:228-305`; extend `PartialAppSettings` and `into_layers` at `settings.rs:679-742`. Change `SettingsStore::materialize`, direct `AppSettings::from_json`, `AppSettings::default`, and `+show-config` materialization to call the single theme-aware resolver. The generic CLI parser at `settings.rs:533-643` stays generic.
5. `crates/mr-crabs-app/src/startup_art.rs`, new, 100 to 140 lines. Own the built-in registry, `LoadedArt`, custom path loading, the 64 KiB cap, newline normalization, cell measurement, and registry/loader tests.
6. `crates/mr-crabs-app/src/lib.rs`, 1 to 3 lines. Declare the art module for the binary helper and app model.
7. `crates/mr-crabs-app/src/animated_fetch.rs`, 180 to 260 lines. Preserve `line.info` from `parse_fetch_layout` at `animated_fetch.rs:143-164`. Add `PresentationLayout` and one arrangement composition point. Convert the five logo-only renderers at `animated_fetch.rs:23-320` to consume composed lines and art spans. Read startup presentation environment values in `run_animated_fetch_and_exit` at `animated_fetch.rs:443-470`. Keep the four fallback classes and add below, beside, info-retention, fit, Unicode-width, and static-Molt tests.
8. `crates/mr-crabs-app/src/model/pane.rs`, 15 to 25 lines. Add one pending-only method that inserts the reserved startup art, arrangement, and animation values into the existing `PtySpawnConfig.env` at `pane.rs:43-55, 998-1003`. No PTY crate change is needed because the environment map already exists.
9. `crates/mr-crabs-app/src/model/window.rs`, 45 to 70 lines. Add `MoltWaiting`, start-on-content transition, and tests around the existing `MOLT_DURATION_MS`, `MoltLayout`, and `StartupPresentation` at `window.rs:43-120`. Keep the state as one enum. Do not add `art_ready` or `molt_started` booleans.
10. `crates/mr-crabs-app/src/model/app_model.rs`, 70 to 110 lines. Resolve the effective presentation in `apply_startup_config` at `app_model.rs:436-453`; activate the built-in startup command for both Rustfetch and Molt when `startup_fetch` permits it; populate pending env; and start waiting Molt on the first frame during the pump at `app_model.rs:640-710`. Preserve `None`, retained Rustfetch dismissal, custom startup commands, GIF setup, and future-window-only startup reload behavior. Extend construction, precedence, dismissal, and first-frame timing tests.
11. `crates/mr-crabs-app/src/ui/workspace.rs`, 25 to 45 lines. Replace the `resolve_chrome` call at `workspace.rs:339-373`, gate dock reserve at `workspace.rs:392-403`, and gate `FocusedDockRender` plus overlay construction at `workspace.rs:484-489, 784-904`. Keep the GPUI Molt paint block at `workspace.rs:676-763`; it continues to paint over the terminal.
12. `crates/mr-crabs-app/src/ui/shell.rs`, 5 to 15 lines. Keep the current combined fetch/Molt scheduler at `shell.rs:206-253`; adjust only if the first-frame transition needs an immediate re-arm assertion. Add a scheduler test proving `MoltWaiting` has no 16 ms loop and `MoltActive` does.
13. `README.md`, 30 to 45 lines. Add the three new CLI/JSON keys near `README.md:119-121`, document theme-default precedence, the built-in/file art grammar, below versus beside, and the revised Molt composition near `README.md:160-163`.

Expected implementation size is about 880 to 1,250 changed or added lines, with roughly one third in tests and the Apple asset. This candidate is larger than a setting-only design because it makes theme behavior real, fixes the discarded rustfetch information, and coordinates Molt with terminal content. It still uses one config chain, one theme table, one arrangement branch, and the existing terminal plus GPUI render paths.

## 7. Verification plan

The implementation should prove these observable contracts:

1. Config unit tests show theme defaults below every explicit layer. Required case: runtime selects Ember, file sets inline, and effective prompt remains inline. Companion cases show CLI beating file and runtime beating CLI.
2. Theme tests iterate `THEMES`, require unique IDs and aliases, resolve every row, and prove `auto` changes only `palette_theme` across light and dark appearance.
3. Art tests load embedded Apple without allocation of its text, load a temporary custom UTF-8 file, reject an unknown built-in ID, and enforce the size cap.
4. Fetch golden tests use the current parsed sample and prove every info line appears in both arrangements. Below must place its first info row after the final art row. Beside must preserve a two-cell gap. Fit and prompt-row calculations must use composed dimensions.
5. Startup state tests prove Molt remains fully masked before the first frame, starts at that frame's timestamp, produces intermediate 16 ms deadlines, completes at 900 ms, and still dismisses on submitted Enter without swallowing the Enter.
6. Workspace behavior tests prove Inline reserves zero dock pixels and creates no dock hit route, while Dock retains the existing 87 px reservation and dock input behavior. A reload from Dock to Inline must change geometry on the next render without changing pane semantic state.
7. CLI tests prove `+show-config` displays the effective theme-derived values and explicit overrides with kebab-case keys; JSON uses snake_case fields.
8. Run the focused config and app tests, then launch the actual app twice. One run uses the default Molt, Apple, and below layout. The other uses `prompt-presentation=inline`, `startup-art=file:<fixture>`, and `fetch-arrangement=beside`. Record the 900 ms startup to verify temporal composition and type a command in each prompt mode to prove the real input path.

## 8. Three biggest risks

1. **A theme now changes more than color.** A user who previously set `theme = "ember"` opted into colors only. After this change, any Ember presentation default also applies unless the user has an explicit override. The compatibility table keeps Dock and Molt for all current themes, but the requested Apple art and below layout still change startup output. Later edits to a theme row could change prompt or startup behavior on upgrade. That is the central cost of theme ownership.
2. **Custom startup commands are opaque.** Mr Crabs can guarantee art replacement and below/beside composition for its built-in `+animated-fetch` helper. An existing `startup_fetch_command` may print arbitrary bytes, ignore the reserved environment values, omit rustfetch's separator, or manage the cursor itself. Reformatting arbitrary output would corrupt valid custom commands, so candidate A preserves them and cannot promise theme layout inside them.
3. **Molt now waits on terminal content.** Starting the clock on the first frame prevents a blank reveal, but a slow or hung custom startup command leaves a black window longer than today's fixed 900 ms. Resize during composition can also force the static fallback. The state machine and composed fit tests contain the normal cases; a timeout policy would be a separate product decision and should not be smuggled into this change.

## Recommendation

Candidate A is internally coherent: a theme is a profile with colors and presentation defaults, while explicit settings remain authoritative. The registry and the two enums remove branch growth rather than hiding it behind layers. Its weak point is migration semantics. Once theme rows own behavior, changing a row becomes a user-visible settings change, not a palette tweak.