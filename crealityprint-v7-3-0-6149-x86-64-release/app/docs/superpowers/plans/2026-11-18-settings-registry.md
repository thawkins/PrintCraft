# Settings Registry (M2.1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One table describes every print setting, and both the settings panel rows and the project file's `"settings"` block are generated from it.

**Architecture:** A new module `src/settings_registry.rs` holds a static table of `Setting` entries, one per `PrintSettings` value. Each entry carries its JSON key, label, hint, group, level (simple or advanced), widget kind with range and step, and accessor functions (`get` and `set` through a small `Value` enum). `PrintSettings` stays a plain struct, so the slicer and G-code code don't change. `project.rs` saves and loads the settings block by walking the table. A new `src/app/settings_ui.rs` draws each group's rows from the table, replacing the hand-written group functions in `prepare_ui.rs`.

**Tech Stack:** Rust 2021, egui/eframe 0.36, serde_json 1.

**Spec:** `docs/PCGAPPLAN.md`, task M2.1 ("Settings registry ... Every existing setting round-trips through a project from the registry. The panel looks the same (screenshot).")

## Global Constraints

- Work in `crealityprint-v7-3-0-6149-x86-64-release/app`, on branch `main` (the user's working branch).
- The crate is binary-only (no `lib.rs`), so tests stay in the existing inline `#[cfg(test)] mod tests` of the module under test. A new module gets its tests in a `#[cfg(test)] mod tests` block at its end, matching every other module here.
- `cargo fmt` after every `.rs` edit. `cargo clippy --all-targets` must have zero warnings. Run tests with `timeout 1500 cargo nextest run`.
- No `.unwrap()`/`.expect()` on user-facing paths. Match enums exhaustively (no `_` arms on project enums).
- G-code must stay byte-identical: `PrintSettings`, `Quality` and the slicer are not touched.
- The project JSON format stays version 1: same keys, nesting and value types. Older and newer files open as before: a missing key gives the default, out-of-range numbers are clamped, unknown choice labels give the default.
- Stage files by name; never `git add -A` (the user keeps untracked files out). Commit only; do not push.

## Review Focus

1. **A project saved by 0.0.6 opens with identical settings.** Pinned by Task 1's golden test, which runs before and after the refactor.
2. **A hand-edited file with out-of-range or wrong-typed values** (string where a number belongs, `-3` for a count, `9999` for a feature speed) loads clamped or defaulted, never panics. Covered in Task 3.
3. **Adding a field to `PrintSettings` without a registry entry** would silently drop it from projects. Task 2's exhaustive struct-literal test fails to compile until both are updated.
4. **Conditional rows** (support options only with supports on; Fuzzy top amount only with Fuzzy top on; "Raft under support" disabled with a different hint under a full raft) must keep behaving. Covered by Task 2's `shown`/`enabled` tests and the Task 4 screenshots.
5. **Quality preset detection after an edit** (`Quality::matches`) must still run when any registry-drawn row changes. Task 4 keeps the `before != after` check around the generated rows.

---

### Task 1: Pin the current settings JSON (characterization)

**Files:**
- Modify: `src/project.rs` (tests module only)

**Interfaces:**
- Consumes: `project::to_json`, `project::from_json`, `tests::sample()` (existing).
- Produces: test `test_project_settings_json_is_unchanged`, which later tasks must keep passing unchanged.

- [ ] **Step 1: Write the test.** Take `sample()`, set every print setting to a non-default value (one field each; include `adhesion: Raft`, `infill_pattern: Triangles`, `seam: Rear`, two feature speeds and one feature acceleration set, the rest `None`). Parse `to_json` output, take `["settings"]`, and `assert_eq!` it against a `serde_json::json!({...})` literal written out in full (every key, nested `feature_speeds` and `feature_accels` with `null`s). Then `from_json` the same text and `assert_eq!(back.settings, p.settings)`. Also load the literal `{"format":"printcraft-project","version":1,"settings":{"walls":3,"seam":"Nowhere","feature_speeds":{"outer_wall":"fast"}}}` and assert `walls == 3`, `seam == default`, `outer_wall` speed `None`.
- [ ] **Step 2: Run it on the current code.** `timeout 1500 cargo nextest run -E 'test(test_project_settings_json_is_unchanged)'`. Expected: PASS. It characterizes today's format; if it fails, fix the literal, not the code.
- [ ] **Step 3: Commit.** `git add src/project.rs && git commit -m "Pin the project settings JSON format with a golden test"`

### Task 2: The registry

**Files:**
- Create: `src/settings_registry.rs`
- Modify: `src/main.rs` (add `mod settings_registry;`)

**Interfaces:**
- Consumes: `PrintSettings`, `Adhesion`, `InfillPattern`, `SeamPosition`, `SpeedFeature`, `AccelFeature`, `limits::*` from `src/profiles.rs`.
- Produces (all `pub`):
  - `enum Group { LayerHeight, Seam, WallsAndSurfaces, Infill, Support, Adhesion, Speed, CoolingAndTravel, SpecialMode }` with `const ALL` (panel order) and `fn title(self) -> &'static str` (the current titles: "Layer height", "Seam", "Walls and surfaces", "Infill", "Support", "Adhesion", "Speed", "Cooling and travel", "Special mode").
  - `enum Level { Simple, Advanced }`.
  - `enum Value { Num(f32), Count(u32), Flag(bool), Choice(&'static str), Auto(Option<f32>) }`.
  - `struct AutoContext { pub machine_accel: f32 }`.
  - `enum Kind { Number { range: RangeInclusive<f32>, step: f64, suffix: &'static str, decimals: Option<usize> }, Count { range: RangeInclusive<u32> }, Flag, Choice { labels: fn() -> Vec<&'static str> }, Auto { range: RangeInclusive<f32>, step: f64, suffix: &'static str, auto: fn(&PrintSettings, &AutoContext) -> f32 } }`. Millimetre fields use `Number` with `suffix: " mm"` and `decimals: Some(2)` (today's `mm()` helper).
  - `struct Setting { pub key: &'static str, pub label: &'static str, pub group: Group, pub level: Level, pub kind: Kind, pub hint: fn(&PrintSettings) -> Option<&'static str>, pub shown: fn(&PrintSettings) -> bool, pub enabled: fn(&PrintSettings) -> bool, pub get: fn(&PrintSettings) -> Value, pub set: fn(&mut PrintSettings, Value) }`. Its `key` is the JSON path, with `.` for nesting (`"feature_speeds.outer_wall"`).
  - `impl Setting { pub fn clamp(&self, v: Value) -> Value; pub fn default_value(&self) -> Value }`. `clamp` limits Num/Count/Auto to the range and maps an unknown `Choice` label to the default.
  - `pub fn all() -> &'static [Setting]` (built once with `std::sync::OnceLock`); `pub fn find(key: &str) -> Option<&'static Setting>`.
- Entry order **is** panel order: within each group, the rows in the order `prepare_ui.rs` draws them today. Labels, hints, ranges, steps, suffixes and decimals are copied from the current group functions. The only conditional rows: support angle, on plate only and raft under support use `shown: supports on`; fuzzy top amount uses `shown: fuzzy_top on`; raft under support uses `enabled: adhesion != Raft` with its two hints.

- [ ] **Step 1: Write the tests** in the new module's `#[cfg(test)] mod tests`:
  - `test_registry_covers_every_field`: build a `PrintSettings` with a struct literal naming **every** field (no `..`), each different from the default. For each `Setting`, `(s.set)(&mut d, (s.get)(&full))` onto a default value `d`. Then `assert_eq!(d, full)`.
  - `test_registry_keys_unique_and_defaults_match`: keys are unique; `default_value()` equals `get(&PrintSettings::default())` for every entry.
  - `test_registry_clamps_and_rejects_unknown_choices`: `find("walls")` clamps `Count(0)` to 1 and `Count(99)` to 10; `find("feature_speeds.outer_wall")` clamps `Auto(Some(9999.0))` to 500 and keeps `Auto(None)`; `find("seam")` maps `Choice("Nowhere")` to the default label.
  - `test_registry_conditional_rows`: with supports off, `shown` is false for `support_angle`, `supports_on_plate_only` and `support_raft`; with adhesion Raft, `enabled` is false for `support_raft`; `fuzzy_top_mm` is shown only with `fuzzy_top`.
- [ ] **Step 2: Run them.** Expected: they fail to compile (module missing).
- [ ] **Step 3: Implement `src/settings_registry.rs`** with a module docblock and `///` on every public item.
- [ ] **Step 4: Run the tests**, then `cargo clippy --all-targets`. Expected: the 4 new tests pass and there are 0 warnings.
- [ ] **Step 5: Commit.** `git add src/settings_registry.rs src/main.rs && git commit -m "Add a registry describing every print setting"`

### Task 3: Project save and load from the registry

**Files:**
- Modify: `src/project.rs` (the `"settings"` part of `to_json` and of `project_from`; their imports)

**Interfaces:**
- Consumes: `settings_registry::{all, Value}` and `Setting::clamp`.
- Produces: `fn settings_json(s: &PrintSettings) -> serde_json::Value` and `fn settings_from_json(v: &serde_json::Value) -> PrintSettings`, both private to `project.rs`.

- [ ] **Step 1: Add the tests.** `test_project_settings_bad_values_load_safely`: a settings block with `"walls": -3`, `"layer_height": "thick"`, `"supports": 1`, `"feature_accels": {"travel": 99999}` and `"adhesion": 7`. It loads without panicking: walls clamped to 1, layer height at its default, supports at its default, travel acceleration 30000, adhesion at its default.
- [ ] **Step 2: Run it** on the current code. Expected: PASS (today's loader already behaves this way; this pins it).
- [ ] **Step 3: Implement** `settings_json`, which walks `all()` and writes each value at its dotted path: `Num` as a number, `Count` as an integer, `Flag` as a bool, `Choice` as a string, `Auto` as a number or `null`. Implement `settings_from_json`, which starts from `PrintSettings::default()` and, for each entry, reads its path, converts it to the entry's `Value` kind (a wrong JSON type counts as missing), clamps it, and sets it. Remove the hand-written settings block, the feature-speed/accel loops and the imports this leaves unused.
- [ ] **Step 4: Run the full suite.** `timeout 1500 cargo nextest run`. Expected: all pass, including Task 1's golden test unchanged.
- [ ] **Step 5: Commit.** `git add src/project.rs && git commit -m "Save and load project settings through the registry"`

### Task 4: Settings panel from the registry

**Files:**
- Create: `src/app/settings_ui.rs`
- Modify: `src/app.rs` (add `mod settings_ui;`), `src/app/prepare_ui.rs` (`process_settings` and the group functions)

**Interfaces:**
- Consumes: `settings_registry::{all, Group, Level, Kind, Value, AutoContext}`, plus the existing `group()` frame and `widgets::row`.
- Produces: `pub(super) fn settings_group(ui: &mut Ui, s: &mut PrintSettings, g: Group, advanced: bool, ctx: &AutoContext)`. It draws the group with `group()` only when it has at least one visible row (Advanced rows need `advanced`, and `shown(s)` must be true).

- [ ] **Step 1: Take "before" screenshots** with the current build: simple mode, and Advanced with supports on and fuzzy top on, using the temporary `advanced: true` override and a `target/temp/shot-target` build, as in earlier milestones. Save them under `target/temp/m21-before-*.png`.
- [ ] **Step 2: Implement `settings_group`.** Each kind maps to today's widget:
  - `Number`: a `DragValue` with range, step, suffix and decimals.
  - `Count`: a `DragValue` with range.
  - `Flag`: `Checkbox::without_text`.
  - `Choice`: a `ComboBox` with `id_salt(key)`.
  - `Auto`: today's `auto_row`, moved here.

  Label hover text comes from `hint(s)`; a disabled row uses `add_enabled` and also shows the hint on hover. In `process_settings`, replace the group calls with `settings_group` for each `Group::ALL` entry, keeping `two_nozzle_group` between Support and Adhesion, and keeping the "Turn on Advanced ..." label and the `Quality::matches` check. Delete the old group functions, `auto_row` and `AutoUnit`, and any helpers left unused.
- [ ] **Step 3: Take "after" screenshots** of the same states. Compare them with a pixel diff (`python3 -I` with PIL: `ImageChops.difference(...).getbbox()`). Expected: no difference in the right-hand panel, or only sub-pixel checkbox spacing; describe any difference in the commit.
- [ ] **Step 4: Run the suite and clippy.** Expected: all tests pass, 0 warnings, `cargo fmt --check` clean.
- [ ] **Step 5: Commit.** `git add src/app/settings_ui.rs src/app.rs src/app/prepare_ui.rs && git commit -m "Draw the print settings panel from the registry"`

### Task 5: Verify G-code, update docs

**Files:**
- Modify: `CHANGELOG.md` (Unreleased > Changed), `docs/PCGAPPLAN.md` (mark M2.1 done)

- [ ] **Step 1: G-code baseline.** Use the existing worktree method (`target/temp/baseline_test2.rs` appended to `src/gcode.rs` in both trees, `BASELINE_OUT`, then restore). Compare the cube, cylinder, XYZ and ORBIT outputs, with and without supports, against commit `4a59f0c` (v0.0.6). Expected: all 8 `SAME`.
- [ ] **Step 2: Open the test project** (`assets/models/bridge-overhang-test.printcraft`) in the app with `--preview --screenshot`. Expected: it slices to 137 layers as before.
- [ ] **Step 3: Update the docs.** CHANGELOG under Unreleased > Changed: "Print settings are described once, in `src/settings_registry.rs`; the settings panel and the project file's settings are generated from it. No change to the panel, project files or G-code." In PCGAPPLAN, mark M2.1 **Done.**
- [ ] **Step 4: Commit.** `git add CHANGELOG.md docs/PCGAPPLAN.md && git commit -m "Mark M2.1 done"`
