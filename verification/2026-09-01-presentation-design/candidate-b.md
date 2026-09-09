# Candidate B: setting-first presentation with theme presets

## Verdict

Candidate B makes presentation a family of ordinary settings. The existing `theme` setting remains the color selector, and its name also chooses a preset that supplies defaults for presentation settings the user did not set.

This satisfies the loose reading of "part of the themes": choosing `paper`, for example, can seed a classic inline prompt. It compromises the strong reading. Presentation is not owned by `ThemeId`, and an explicit presentation value survives a later theme change. If the requirement means that a theme object must contain and always control prompt, startup, art, and fetch behavior, this candidate does not satisfy it. Candidate A is the honest fit for that interpretation.

The compromise is intentional. It keeps color resolution at `theme.rs:57-91` free of behavioral branches and lets users combine any color theme with the presentation they want.

## Data shape first

These are the user-facing keys. JSON keeps the repository's snake_case convention, and CLI names keep the existing kebab-case convention.

| CLI key | JSON field | Typed value | Values |
| --- | --- | --- | --- |
| `prompt-presentation` | `prompt_presentation` | `PromptPresentation` | `dock`, `inline` |
| `startup-animation` | `startup_animation` | existing `StartupAnimation` | `none`, `rustfetch`, `molt` |
| `startup-art` | `startup_art` | `StartupArt` | `none`, `native`, `apple`, `file:/absolute/path` |
| `fetch-arrangement` | `fetch_arrangement` | `FetchArrangement` | `below`, `beside`, `hidden` |

There is no `use-input-dock` boolean and no separate `custom-art-path`. A second path field would let `startup_art = custom` coexist with an empty or stale path. `StartupArt::Custom(PathBuf)` carries the path in the selected variant instead.

The config crate adds these types beside the existing `StartupAnimation` enum at `crates/mr-crabs-config/src/lib.rs:68-99`:

```rust
pub enum PromptPresentation {
    Dock,
    Inline,
}

pub enum StartupArt {
    None,
    Native,
    Apple,
    Custom(PathBuf),
}

pub enum FetchArrangement {
    Below,
    Beside,
    Hidden,
}

```

The four independent raw keys resolve into a discriminated runtime shape:

```rust
pub struct PresentationSettings {
    pub prompt: PromptPresentation,
    pub startup: StartupSettings,
}

pub enum StartupSettings {
    None,
    Rustfetch {
        art: StartupArt,
        arrangement: FetchArrangement,
    },
    Molt {
        art: StaticStartupArt,
    },
}

pub enum StaticStartupArt {
    None,
    Apple,
    Custom(PathBuf),
}
```

`PresentationSettings` is the runtime value. Rendering and window construction receive it rather than four strings or a theme ID. `Molt` cannot carry native rustfetch art, and `Rustfetch` always carries an arrangement. The extra enums encode those two real constraints and remove downstream validation branches.

Each raw setting enum has the same responsibilities as the existing enum setting: a strict parser, a canonical config spelling, a default, and one typed accessor from `EffectiveConfig`. The associated path makes `StartupArt` non-`Copy`, but it is still one enum-valued setting. `file:` must contain an absolute path. Requiring an absolute path avoids threading config-file origin through `ConfigOverlay`, CLI, runtime overlays, and child processes merely to interpret one value.

The four values have these meanings:

* `prompt = Dock` enables the current semantic input dock. `Inline` leaves the PTY grid and prompt row untouched and reserves no dock chrome.
* `startup_animation = None` resolves to `StartupSettings::None`. The other raw startup fields remain configured for a later animation change.
* `startup_animation = Rustfetch` resolves to `StartupSettings::Rustfetch` and keeps the existing startup command and retained Enter behavior. The default `+animated-fetch` path reads the selected art and arrangement from pane environment variables. A user-supplied `startup_fetch_command` still runs as supplied.
* `startup_animation = Molt` resolves to `StartupSettings::Molt` and ends on the selected static art. `native` is rejected during settings materialization because the native rustfetch logo only exists after rustfetch capture. `none`, `apple`, and `file:` become `StaticStartupArt` variants.
* `startup_art = Native` means the logo parsed from rustfetch. `Apple` and `Custom` replace the parsed logo while keeping parsed system information. `None` gives an info-only fetch or the old hole-only Molt.
* `fetch_arrangement = Below` puts system information under the art. `Beside` reconstructs the traditional rustfetch layout. `Hidden` emits only art.

Raw configuration can contain `molt` plus `native`, but a successful runtime snapshot cannot. `AppSettings::from_effective` resolves the four values into `PresentationSettings` and returns an error for that combination. This changes settings materialization from infallible to fallible, while `SettingsStore::commit` keeps the existing atomic swap rule.

This follows the repository's fixed setting path rather than inventing another config route. `SettingKey`, `ConfigOverlay`, `EffectiveConfig`, and `AppSettings` already carry enum settings through the sites listed in the established facts. The generic CLI parser at `settings.rs:533-643` remains generic.

## Preset mechanism

A preset is data:

```rust
pub struct PresentationPreset {
    pub theme: &'static str,
    pub prompt: PromptPresentation,
    pub startup_animation: StartupAnimation,
    pub startup_art: StartupArt,
    pub fetch_arrangement: FetchArrangement,
}
```

The preset table lives in `mr-crabs-config`, next to defaults and overlay resolution. It does not live in `theme.rs`. That location matters because the preset participates in `defaults < file < cli < runtime`, while `theme.rs` only maps a resolved color name to `TerminalPalette` and `WindowMaterial` at render time.

Presets are keyed by the existing canonical theme names. They are not a new user-facing concept, and there is no `presentation-preset` key. A user applies one with the existing `theme` setting:

```json
{
  "theme": "paper"
}
```

or:

```text
mr-crabs --theme paper
```

Resolution works in this order:

1. Resolve the canonical theme name from runtime, CLI, file, then `DEFAULT_THEME`.
2. Look up one `PresentationPreset` by that name and use it as the base for the four presentation values.
3. Apply explicit file, CLI, and runtime presentation fields in the existing order.
4. Validate and build one validated `PresentationSettings` once.

A preset never writes into the user's config. It supplies defaults on each materialization. An explicit file value therefore beats a preset selected by CLI or runtime. That is what "seeds" means here. If a user wants the theme's prompt or startup choice again, they remove the explicit presentation field.

`auto` has its own preset. It does not switch presentation when macOS appearance changes. `resolve_chrome` may continue mapping `auto` to Ink or Paper colors based on `WindowAppearance` at `theme.rs:64-70`, but a system appearance change must not resize the terminal or replace a live prompt presentation.

Initial preset data:

| Theme | Prompt | Startup | Art | Fetch |
| --- | --- | --- | --- | --- |
| `auto` | `dock` | `molt` | `apple` | `below` |
| `ink` | `dock` | `molt` | `apple` | `below` |
| `paper` | `inline` | `molt` | `apple` | `below` |
| `harbor` | `dock` | `molt` | `apple` | `below` |
| `ember` | `dock` | `molt` | `apple` | `below` |

Only Paper opts into the classic prompt. A color name does not justify inventing more behavioral differences. The table can change as data if product design later assigns them.

A table-completeness test compares its names with the accepted canonical theme names. This catches drift without moving `ThemeId` out of the app or teaching `resolve_chrome` about presentation.

## Art registry

The app adds one small data owner, `startup_art.rs`:

```rust
pub struct AsciiArt {
    pub id: Arc<str>,
    pub lines: Arc<[ArtLine]>,
    pub width_cells: u16,
    pub height_rows: u16,
}

pub struct ArtLine {
    pub text: Arc<str>,
    pub width_cells: u16,
}

pub struct BuiltinArt {
    pub name: &'static str,
    pub source: &'static str,
}

pub static BUILTIN_ART: &[BuiltinArt] = &[
    BuiltinArt {
        name: "apple",
        source: include_str!("../assets/startup-art/apple.txt"),
    },
];
```

`ArtRegistry::resolve` handles `None`, a built-in lookup, and a custom file. `Native` is resolved from `FetchLayout.lines[*].logo` by `animated_fetch.rs`, because rustfetch art is runtime data rather than a built-in asset. That matches the current parser at `animated_fetch.rs:143-164` instead of copying captured art into the registry.

Built-in Apple art is plain UTF-8 in `crates/mr-crabs-app/assets/startup-art/apple.txt` and compiled with `include_str!`. Embedding makes the installed app independent of a resource path and keeps the logo versioned with the binary. The cost is a slightly larger binary and a rebuild for art edits. For one small logo, embedding is the better trade.

Custom art stays path-backed. `file:/absolute/path` is validated syntactically when settings materialize, then loaded when a future window resolves `StartupSettings::Molt`. The loader caps input at 64 KiB, normalizes CRLF, and converts it into `AsciiArt`. It rejects NUL, terminal control sequences, tabs, zero-line files, dimensions that exceed `u16`, and invalid UTF-8. It computes cell widths once for that window, so paint and fit checks do not rescan every glyph on every frame. A failed custom load reports the path error and starts the terminal with no startup art; it does not replace the selected setting or block the shell. Built-in art is parsed once through `OnceLock`.

The registry stores art only. It knows nothing about themes, animation time, GPUI, or fetch arrangement.

## Fetch composition

`FetchLayout` already preserves `FetchLine { logo, info }` at `animated_fetch.rs:11-21`. The loss happens later: `logo_only_bytes`, `frame_bytes`, `positioned_frame`, `dimmed_frame`, and `centered_animation_chunks` emit only `line.logo`.

Replace those per-renderer decisions with one arrangement function:

```rust
pub struct FetchContent {
    pub art: Arc<AsciiArt>,
    pub info: Arc<[String]>,
}

pub struct ArrangedFetch {
    pub rows: Arc<[ArrangedRow]>,
    pub width_cells: u16,
    pub height_rows: u16,
}

pub struct ArrangedRow {
    pub art: Option<ArtSpan>,
    pub info: Option<String>,
}

pub fn arrange_fetch(
    content: &FetchContent,
    arrangement: FetchArrangement,
) -> ArrangedFetch;
```

`Below` emits all art rows, one blank separator row when both sections exist, then nonempty info rows with the original left padding removed. `Beside` zips art and info rows and inserts two cells between them. `Hidden` omits info. `ArrangedRow` retains the art span, so the existing rainbow and dim phases color only art while leaving system information readable.

All five current output functions consume `ArrangedFetch`. None reads `FetchLine.logo` directly. This removes the five ways information is currently discarded. Parse-failure and does-not-fit fallbacks at `animated_fetch.rs:443-470` remain. A parse failure still writes the original rustfetch output, which already contains its side-by-side information. A fit failure writes the arranged static block rather than reverting to logo-only output.

The default startup command remains valid. `AppModel` adds the canonical art and arrangement values to the pane environment before spawn. The `+animated-fetch` child reads those values. An arbitrary `startup_fetch_command` sees harmless extra environment variables and otherwise runs unchanged.

## Molt and art composition

The existing ownership stays recognizable:

* `model/window.rs` owns time, phase, and the data required to paint one frame.
* `AppModel::next_molt_deadline_ms` and `tick_molt_animations` at `model/app_model.rs:710-745` own the 16 ms repaint schedule and the completion transition.
* `ui/workspace.rs` owns GPUI painting. Its current overlay pass at `workspace.rs:676-763` remains the only Molt renderer.

`StartupPresentation` stops being `Copy` and carries the resolved art in the state that needs it:

```rust
pub enum StartupPresentation {
    None,
    RustfetchRetained,
    RustfetchDismissed,
    MoltActive {
        started_ms: u64,
        art: Option<Arc<AsciiArt>>,
    },
    MoltRetained {
        art: Option<Arc<AsciiArt>>,
    },
    Dismissed,
}
```

There is no separate `molt_complete` boolean and no window-level art field that can drift away from the phase. At 900 ms, `MoltActive` moves its art into `MoltRetained`. A forwarded Enter changes either Molt state to `Dismissed`; the Enter still reaches the PTY. `RustfetchRetained` keeps its current dismissal path.

For each active frame, `model/window.rs` returns a `MoltFrame` with the existing sine-out progress, hole rectangle, and glow alpha plus two new values:

```rust
pub struct MoltFrame {
    pub eased: f32,
    pub hole: RectF,
    pub glow_alpha: f32,
    pub art_alpha: f32,
    pub art_clip: RectF,
    pub art: Option<Arc<AsciiArt>>,
}
```

The composition is concrete:

1. Lay out the art once as a centered, monospace block using the terminal font and cell metrics.
2. Paint the existing four black rectangles outside the expanding center hole.
3. Clip the art to the hole. Its alpha ramps from 0 to 1 over eased progress 0.15 through 0.75. At the start there is no visible art. As the hole opens, more Apple rows and columns become visible and the glyphs fade in.
4. Paint the existing cyan edge strips with `glow_alpha` over the mask and art.
5. At 900 ms, remove the mask and glow but retain the fully opaque art until Enter.

`MoltRetained` produces a static art frame and requests no animation deadline. `startup_art = none` preserves the old hole-only dissolve. No shader, Metal code, second renderer, or terminal UI crate enters this path. GPUI `div` clipping and text paint are enough.

The animation program stays 900 ms with 16 ms target frames as defined at `model/window.rs:43-120`. This design changes composition, not duration or easing.

## Prompt presentation

The semantic dock keeps its current model and paint code. The mode is selected once in `workspace.rs` from the current `PresentationSettings` snapshot:

```rust
let use_dock = presentation.prompt == PromptPresentation::Dock;
```

That value gates both existing decisions:

* `SurfaceGeometry::from_viewport_with_dock_reserve` receives `use_dock && pane.should_reserve_dock_chrome()` at `workspace.rs:392-396`.
* `focused_dock` is built only when `use_dock` at `workspace.rs:484-500`.

The paint, mouse, IME, and footer behavior downstream of `focused_dock` remains one path. `accessibility.rs` may continue reading the cached semantic snapshot independently, so inline mode does not lose the current prompt value. Inline mode does not create a second prompt renderer. It simply exposes the terminal's existing prompt row and uses the full terminal height. The derived dock snapshot may remain cached in the pane so switching the setting on reload does not require copying prompt text or adding pane-local mode state.

Prompt presentation applies on the next render after reload. Geometry commits the changed reserved height through the existing resize path. Startup animation, art, and arrangement apply only to future windows, matching the current startup-setting behavior described in the established facts.

## Why choose candidate B

### Branches removed

A theme-owned design makes prompt and startup rendering depend on a resolved theme in addition to their own state. Candidate B resolves the theme preset once, before rendering. `resolve_chrome` still branches only on color theme. Workspace branches once on `PromptPresentation`, window startup matches once on `StartupSettings`, and every fetch renderer consumes the same arranged rows.

The largest concrete deletion is in `animated_fetch.rs`: five renderers stop choosing `line.logo` and silently dropping `line.info`. One arrangement function owns below, beside, and hidden.

### Invalid states removed

`PromptPresentation` cannot be both dock and inline. `StartupArt::Custom(PathBuf)` cannot be custom with no path. `StartupSettings::Molt` cannot carry native rustfetch art. Molt phase and retained art travel in one enum variant, so a completed boolean cannot disagree with an art field.

A table lookup replaces theme-by-presentation conditionals. Adding a theme adds one preset row. Adding an art asset adds one registry row. Neither adds a match arm to every renderer.

### Blast radius

Theme color code remains unchanged. `ThemeId`, `resolve_chrome`, `InputDockTokens`, palette constructors, and the GPUI pin do not move. Reload keeps its existing split: prompt mode is read on render, and startup values affect future windows. Candidate B pays the known setting-plumbing cost but avoids threading behavioral fields through `ResolvedChrome` and every caller that only needs colors.

## Why not choose candidate B

The main disadvantage is conceptual, not cosmetic. A theme does not own presentation. It supplies defaults. Users can select Paper and still see the dock because an older explicit `prompt_presentation = "dock"` wins. That behavior is correct for independent settings and surprising if the product promises themes as complete experiences.

The second disadvantage is plumbing volume. Three new keys traverse the full config and app settings path, while `startup-animation` already exists. The design touches many repetitive sites even though the runtime model is small. A theme-owned record can be shorter if customization is limited to built-in themes.

There is also a controlled duplication risk. The preset table is keyed by canonical theme strings while `ThemeId` remains in the app. A completeness test catches drift, but the compiler cannot. Moving `ThemeId` into `mr-crabs-config` would remove that risk at the price of a larger unrelated change. Candidate B deliberately does not make that move.

## Ordered implementation sites and estimates

Estimates are net lines added or materially changed, including focused tests. They are planning estimates, not measured diffs.

1. `crates/mr-crabs-config/src/lib.rs`, about 240 to 300 lines. Add the three new setting enums, `PresentationSettings`, `PresentationPreset`, preset lookup, default constants, and typed parsing. Extend `SettingKey` variant, `ALL`, `flag`, `from_flag`, and `docs`; extend `ConfigOverlay` field, merge, apply, and set; extend `EffectiveConfig` field, theme-seeded defaults, resolve, display, and typed accessor. Add parser, canonicalization, preset precedence, and table-completeness tests. The existing `StartupAnimation` sites stay and join the resolved struct.
2. `crates/mr-crabs-app/assets/startup-art/apple.txt`, about 12 to 20 lines. Add the plain UTF-8 built-in Apple logo.
3. `crates/mr-crabs-app/src/startup_art.rs`, about 130 to 170 lines. Add `AsciiArt`, `ArtLine`, the built-in registry, custom file loading, width caching, validation, and loader tests. This module owns all art data and I/O.
4. `crates/mr-crabs-app/src/lib.rs`, 2 to 4 lines. Register and expose the art module to the binary and model.
5. `crates/mr-crabs-app/src/settings.rs`, about 110 to 150 lines. Add the three raw JSON fields to `PartialAppSettings`; extend `into_layers` and `effective_config`; replace the app-layer startup string accessor with one resolved `PresentationSettings` field; make `from_effective` and store materialization fallible for invalid combinations and `file:` syntax; add JSON, CLI, layer-precedence, reload, and show-config tests. The generic CLI loop stays unchanged.
6. `crates/mr-crabs-app/src/model/window.rs`, about 90 to 130 lines. Replace the copyable Molt variants with art-carrying active and retained variants, return `MoltFrame`, preserve the 900 ms sine-out calculation, and test start, midpoint, completion, retention, and Enter dismissal.
7. `crates/mr-crabs-app/src/model/app_model.rs`, about 70 to 110 lines. Match the validated `StartupSettings` for a new window, pass loaded art into `StartupPresentation`, export canonical art and arrangement values to pane environment for `+animated-fetch`, and adapt deadline, tick, and startup tests to non-`Copy` state. Reload still pushes only current pane-owned animation, cursor, and scrollback settings.
8. `crates/mr-crabs-app/src/animated_fetch.rs`, about 170 to 230 lines. Convert rustfetch logo and chosen art to `FetchContent`; add `arrange_fetch`; make all byte and frame functions consume `ArrangedFetch`; read the parent-provided art and arrangement environment; keep parse and fit fallbacks; add below, beside, hidden, custom-art, non-TTY, and final-prompt-position tests.
9. `crates/mr-crabs-app/src/ui/workspace.rs`, about 100 to 150 lines. Gate reserve and focused dock construction on `PromptPresentation`; replace direct Molt geometry painting with `MoltFrame` composition; paint clipped art and retained art through GPUI; update pointer-route and GPUI visual tests for dock and inline geometry plus Molt start, middle, and retained frames.
10. `README.md`, about 35 to 55 lines. Change the key count from 26 to 29, add the three flags and JSON fields, document preset seeding and override precedence, document art values and absolute custom paths, and update Molt and rustfetch layout behavior.

No changes are planned in `crates/mr-crabs-app/src/theme.rs`, `crates/mr-crabs-app/src/ui/input_dock.rs`, `crates/mr-crabs-app/src/model/input_dock.rs`, `crates/mr-crabs-app/src/model/geometry.rs`, `crates/mr-crabs-app/src/accessibility.rs`, any Cargo manifest, or the GPUI dependency pin. `include_str!` needs no packaging rule or new dependency.

## Verification plan

1. Run config and settings tests that cover strict enum parsing, all 29 `SettingKey` entries, canonical show-config output, theme seed precedence, explicit override precedence, runtime persistence through reload, custom path syntax, missing-file behavior, and the `auto` preset remaining independent of OS appearance.
2. Run `animated_fetch` tests proving every public output path includes information below the art by default, `beside` reconstructs paired rows, `hidden` omits information, non-TTY output follows the arrangement, parse failure retains original output, and the final cursor lands below the whole arranged block.
3. Run model tests proving Molt has no art at time zero, partial clipped art at the midpoint, retained full art at 900 ms, no deadline after retention, and forwarded Enter dismisses both active and retained states without consuming the Enter.
4. Run GPUI visual tests for the same pane in `dock` and `inline` modes. Verify dock mode reserves 87 px and routes dock input; inline mode reserves zero, paints the terminal prompt row, and follows the existing terminal input route.
5. Launch the real app twice with deterministic fixture art and fetch data. Record the 900 ms Molt at start, middle, and completion, then verify Apple art remains until Enter and system information appears below art in Rustfetch mode. Temporal video or frame-sequence evidence is required for the animation claim.
6. Run the existing full workspace suite after focused tests. The implementation changes startup state equality and settings key count, so a focused pass alone is not enough.

## Unresolved risks

* The initial Paper-to-inline mapping is a product choice. It is isolated to one preset row, but it still needs visual approval.
* A user-supplied `startup_fetch_command` is not guaranteed to understand the new environment values. It keeps working exactly as a command, but art replacement and fetch arrangement apply only to the built-in `+animated-fetch` protocol unless that command opts in.
* Retaining Apple art after 900 ms changes Molt's current completion behavior. The requested wording implies a visible destination, but visual review must confirm that Enter-to-dismiss is the intended lifetime.
* Theme-name duplication remains checked by a test rather than encoded by one shared enum. That is the explicit cost of keeping theme color ownership unchanged.

