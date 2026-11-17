# PrintCraft deep-refactor audit

Snapshot: commit `4a62057` (2026-11-17). Every count below is a measurement at
that commit with the command that produced it; re-derive before acting on it.

Baseline at `4a62057`:

| Gate | Command | Result |
|---|---|---|
| Tests | `cargo test` | 87 passed |
| Format | `cargo fmt --check` | clean |
| Compiler | `cargo build 2>&1 \| grep -c warning` | 0 |
| Clippy | `cargo clippy --all-targets` | 12 style warnings (chunks_exact, field_reassign_with_default, sort_by_key, collapsible_if) |
| Size | `wc -l src/*.rs src/*/*.rs` | 12 817 lines; largest `device_ui.rs` 1960, `app.rs` 1296, `slicer.rs` 1089 |

## Architecture verdict

- **Core is clean.** `slicer`, `geometry`, `gcode`, `heat`, `model`, `formats` and
  `thumbnail` import no egui (`rg -l egui src/{slicer,geometry,gcode,heat,model,formats,thumbnail}.rs` → none).
  Minor cycle `model` ↔ `formats`.
- **Printer layer is sound.** An `Api` enum plus a per-module `run(&Worker)`
  contract; a trait would be ceremony for four closed variants. One egui leak:
  `printer/camera.rs` returns `egui::ColorImage`. `calibration.rs`/`layer_info.rs`
  are G-code transforms living in `printer/`.
- **UI is "smart" — the main structural debt.** Plate layout, fit check, line
  width, the slice → G-code → thumbnail pipeline (duplicated in `main.rs`), the
  print-again state machine and G-code post-processing live in egui `impl` blocks.
  `PrintCraftApp` needs an `eframe::CreationContext`, so `app.rs` has zero tests.
  `device_ui` ↔ `app` import cycle via `app::card`.

The debt is UI-held domain logic, duplication and god-functions, not the overall
shape. Structural work goes last (Car 9).

## Defects (verified)

Each was confirmed by reading the cited lines; "locked" means the fix ships with a
test that fails without it.

| ID | Defect | Evidence | Car |
|---|---|---|---|
| D1 | A `.printcraft` file whose mesh contains `====` crashes the app (`3 - pad` underflow); `a===` and mid-string padding are accepted | `project.rs` `base64_decode` | 1 |
| D2 | Non-ASCII char after `=` in 3MF XML panics (byte-1 slice of a multi-byte char) | `formats.rs` tag parser `after[1..]` | 1 |
| D3 | STL/OBJ/3MF accept NaN/inf coordinates → NaN placement, slicer infinite loop | `model.rs` STL parse, `formats.rs` OBJ/3MF | 1 |
| D4 | OBJ face with a bad index silently becomes a different triangle | `formats.rs` OBJ `filter_map` | 1 |
| D5 | Picking hits a model from anywhere if it has one degenerate triangle | `view3d.rs` `inside_triangle` returns true when all signs are 0 | 1 |
| D6 | Deleting / opening a project mid-move-drag panics on `objects[index]` | `app.rs` `Drag::Move` arm | 1 |
| D7 | `slicer::contains` underflows on an empty polygon | `slicer.rs` `n - 1` | 1 |
| D8 | Absolute E kept in `f32`: ~0.03 mm quantisation on 1 kg prints, skews grams/cost | `gcode.rs` `let mut e = 0.0f32` | 2 |
| D9 | 3MF `unit=` ignored (inch/cm files load at wrong scale) | `formats.rs` (only a fixture mentions `unit`) | 2 |
| D10 | Stale `send_after_slice` after a refused slice pops "Start printing?" later | `prepare_ui.rs` `send_print`, `app.rs` `start_slice` | 2 |
| D11 | New/Open project keeps the running slice; its result lands on the new plate | `project_ui.rs` `new_project`/`apply_project` | 2 |
| D12 | Project values not range-checked (walls, speeds 0 → divide by zero in estimate, temps) | `project.rs` `from_json` | 2 |
| D13 | Save As with a dotted name (`v1.2`) replaces the extension → overwrites `v1.printcraft` unasked | `project_ui.rs` `set_extension` | 2 |
| D14 | CLI help omits `aligned` (the default seam) | `main.rs` usage text | 2 |
| D15 | PrusaLink: a 16+ char Digest password is sent in clear as `X-Api-Key` on the first request of every client | `prusalink.rs` `sign()` | 3 |
| D16 | Creality: an upload > 45 s logs "Starting the print" before sending, and the silence check then forces a false "Connection lost" | `creality.rs` `run_command` / `session` | 3 |
| D17 | PrusaLink uploader re-uses the nonce with `nc` reset to 1 | `prusalink.rs` uploader `Client` | 3 |
| D18 | USB `Streamer::stop` leaves `resend_from` set → old moves can replay after Stop | `usb.rs` `stop` | 3 |
| D19 | Only the first resolved address is tried (IPv6/IPv4 localhost mismatch) — 3 copies | `rg -n to_socket_addrs src` → 3 | 3 |
| D20 | Saves are not `fsync`ed before rename; temp left on rename failure — 2 copies | `project.rs` `save`, `known.rs` `save` | 4 |
| D21 | Records tab lists printer-history prints twice | `device_ui.rs` records merge | 2 |
| D22 | Scan spinner spins forever if the discovery thread dies | `device_ui.rs` `poll_scan` | 2 |

## Product decisions needed (status `Blocked — decision`)

| ID | Question |
|---|---|
| Q1 | `MIN_PRINT_SPEED` (15 mm/s) also overrides speeds the user set lower (`gcode.rs` `.max(MIN_PRINT_SPEED)`). Floor only the cooling slowdown? |
| Q2 | Disconnect does not cancel an in-flight upload; the print still starts. Cancel? |
| Q3 | Opening a project from a newer version silently drops unknown fields on save. Warn? |

## Measured / likely perf waste

| ID | Item | How to measure |
|---|---|---|
| P1 | `layer_outline` scans every triangle every layer, allocating a `Vec` per triangle | time the CLI on a large STL before/after; output must be byte-identical |
| P2 | `LinkState` deep-cloned ~7×/frame via `link.state()` | count calls per frame; add a borrowing accessor, keep `state()` (pinned) |
| P3 | `Camera::project` recomputes the view basis per vertex | frame time on a 100k-triangle model |
| P4 | USB loop calls `w.update` (lock + repaint) per received line / ~20 Hz idle | repaint counter |

## Behaviour-preserving refactor backlog

| ID | Change | Canonical owner | Pins |
|---|---|---|---|
| R1 | 3D bounds-of-points loop (`rg -n 'f32::INFINITY' src` → 11 hits) → `Bounds::of` | `geometry.rs` | `Bounds{min,max}` literal in gizmo tests — keep fields |
| R2 | Plate fit / line width / slice pipeline duplicated in `app.rs` and `main.rs` (fit slack drifted: `+0.01` vs none) | new `pipeline.rs` | MECHANICS.md names `app.rs line_width` — update doc |
| R3 | Base64 encode in `thumbnail.rs`, decode in `project.rs` | new `codec.rs` | `crate::thumbnail::base64`, `project::base64_decode` used by tests → keep `pub use` aliases |
| R4 | Atomic write duplicated (`project.rs`, `known.rs`) | new `fsutil.rs` (fixes D20) | none |
| R5 | Printer ⇄ JSON written twice (`project.rs`, `known.rs`); name fallbacks differ and must stay | `printer/mod.rs` | known/project tests on fields |
| R6 | Object actions (turn, lay flat, duplicate, delete) duplicated in `app.rs` and `prepare_ui.rs`, passed as `"dup"`/`"del"` strings | `app.rs` `ObjectAction` enum | none (no app tests) |
| R7 | `app` ↔ `device_ui` cycle via `card` and shared widgets | new `widgets.rs` | none |
| R8 | Upload-and-print / HTTP poll / agent builders repeated across creality, moonraker, prusalink (messages drifted) | `printer/mod.rs` helpers | e2e wire strings in `printer/tests.rs` |
| R9 | `gcode::generate` 238 lines → header / layer / path / footer helpers | `gcode.rs` | byte-identical output (determinism test) |
| R10 | `slicer::slice` 167 lines → walls / infill region / solid-sparse / ironing | `slicer.rs` | layer-count and path-kind tests |
| R11 | UI god-functions (`process_settings` 204, `tabs_card` 160, `current_card` 154, `settings_card` 137, `objects_panel` 130, `view_overlays` 118, `find_card` 114, `handle_view_input` 109) → in-place helpers | same files | device_ui tests' private fields/methods stay on `DevicePage` |
| R12 | Dead `retracted` flag in `gcode::generate`; lying comments (`to_binary_stl` "save model", orphaned doc on `ModelPart`, `INFILL_OVERLAP` "15 % overlap", MECHANICS.md seam default "Rear", `main.rs` `--printer 1-4`) | same files | none |
| R13 | `camera.rs` returns `egui::ColorImage` from `printer/` | return RGBA + size; convert in UI | none |
| R14 | 12 clippy warnings | — | none |

## Deliberate — do not fix

- Two duration formats (`gcode.rs` "1 h 5 min" vs device "01h 05m"): the device one mimics the printer screen and is pinned.
- `Result<_, String>` everywhere, no logging crate: consistent, adequate.
- Enum labels stored in the project file: change only if labels change.
- `allow(unused_mut)` in `main.rs`: macOS-only mutation.
- `rfd` default backend, `md-5`, `chrono`: each justified; duplicate transitive versions come from eframe/winit.
- No printer backend trait: four closed variants, enum dispatch is simpler.
- Re-arranging the plate when adding a model; centroid painter's sort; 200-file/100-record caps.
- PrusaLink overwrites progress with None when idle (pinned by `reads_prusalink_status`).

## Blocked by tests (test-refactor scope)

- `completion_time_uses_the_local_time_zone` mutates `TZ` while parallel tests read local time — a flake source.
- `printer/tests.rs` `read_request` and `read_full_request` are near-duplicates.

## Second audit pass (after car 8)

A fresh pass over the files the first pass skimmed (gizmo, heat, calibration,
layer_info, discovery, digest, model, thumbnail, theme, CLI) was not dry:

| ID | Defect | Car |
|---|---|---|
| N1 | Lay flat summed faces by direction only, so step tops tied with the bottom; ties broken by HashMap order (non-deterministic) | 9 |
| N2 | X/Y rotate rings turn about the model's own axes once it is turned about Z (rotation applied X, then Y, then Z) | open, needs `app.rs` (after car 8b) |
| N3 | Calibration inserted after the nozzle wait even when the plate heats later (cold-plate levelling) | 9 |
| N4 | `BED_MESH_CALIBRATE` in a comment counted as "already calibrating" | 9 |
| N5 | One bad ASCII-STL corner shifted every later triangle | 9 |
| N6 | Huge `TEMPERATURE_WAIT MINIMUM` / `;LAYER:` values overflowed | 9 |
| N7 | CLI help omitted `aligned`; an option was taken as another option's value | 9 |
| N8 | Pressing the gizmo centre grabbed the X arrow; rings bridged points behind the camera | 10 |
| N9 | Digest scheme matched case-sensitively | 9 |

Still open from the second pass (low): `heat_up_only` ignores `T` / lowercase `m109`;
a binary STL whose header starts with `solid` and has trailing bytes is read as ASCII;
discovery probes a found host's ports twice; `discovery::identify` and `link::detect_api`
repeat the same port-order chain.

## Cleanup train ledger

Status at the end of the drain. Local branches `refactor/car-N-*`, fast-forwarded into
local `main`; nothing pushed.

| Car | Scope | Status |
|---|---|---|
| 1 | Crash fixes on untrusted input: D1–D7, 8 locking tests | Done (`1e9891c`) |
| 2 | Output and workflow correctness: D8–D14, D21, D22 | Done (`8e409d4`) |
| 3 | Printer link correctness: D15–D19 | Done (`340b5dc`); D16 (Creality pending start) has no automated test |
| 4 | `fsutil` atomic write with fsync (D20, R4), `codec` base64 (R3) | Done (`3406b3f`) |
| 5 | Core helpers R1, R12, R14, P1 (sphere.stl 0.39 s → 0.24 s, `/usr/bin/time`) | Done (`1ecd305`) |
| 6 | Shared pipeline + object actions + widgets: R2, R6, R7 | Done (`0b5f673`) |
| 7 | Printer helpers and perf: R5, R8, R13, P2, P4 | Done (`72d5da1`); Moonraker/PrusaLink poll-loop logging left different (decision) |
| 8a | `gcode::generate`, `slicer::slice` split; per-frame camera view (R9, R10, P3) | In progress |
| 8b | UI god-functions split in place (R11) | In progress |
| 9 | Second-pass fixes N1, N3–N7, N9 | Done (`d87b002`) |
| 10 | Gizmo N8 | Done (`2760d85`) |
| — | N2 rotate rings (world axes) | Approved, after 8b |
| — | `Plate` model out of `PrintCraftApp`; TZ-mutating test | Proposed: needs test edits enumerated |
| — | Q1–Q3 | Blocked — decision |
| — | Moonraker vs PrusaLink contact-error logging | Blocked — decision |

## Lessons for the next audit

- A worktree agent starts from the repository's pushed base, not the local branch:
  hand it the exact base commit and check `git merge-base` before trusting its counts.
- `sed`/string revert probes can hit the wrong copy of a repeated snippet (the USB
  non-vacuity check first reverted `start()` instead of `stop()`); anchor them.
- A locking test can pass for the wrong reason (lay flat: a tie the new tie-break
  settled alone); prove each part of a fix with its own non-vacuity check.
