# PrintCraft gap-closing plan

Date: 2026-11-18. Baseline: PrintCraft 0.0.5. The gaps come from `PCGAP.md`. The
numbers in brackets, such as [#1], are the rows of its gap ranking (section 4).

## Rules for every task

1. **Keep the simple view simple.** A new setting goes in a collapsed "Advanced" group
   unless it changes most prints. Defaults must keep today's output where possible.
2. **Single-nozzle baseline.** When a task changes G-code, compare a fixed set of test
   projects against the previous output. Any difference must be intended and listed in
   the commit message. The test projects are a cube, the ORBIT fixture, an overhang
   test and a bridge test.
3. **Done means:**
   * tests in `tests/` (as AGENTS-LANGUAGE.md requires) pass under `cargo nextest run`;
   * `cargo clippy --all-targets` passes with no warnings;
   * `cargo fmt --check` passes;
   * `MECHANICS.md`, `mechanics.json` and `FEATURES.md` are updated;
   * UI changes have a screenshot check (`--screenshot`).
4. **Project files.** Every new setting is saved and loaded in `.printcraft` files.
   Missing values fall back to defaults, so older files still open.
5. Estimates are working days for one developer (with an agent). Each task is one
   commit, or one PR.

## Milestone overview

| Milestone | Theme | Gaps closed | Estimate | Version |
|---|---|---|---|---|
| M1 | Print-quality core | #1, #2, #5, #7 | 10-12 days | 0.1.0 |
| M2 | Control and customisation | #3, #6, #8, #17 (part) | 8-10 days | 0.2.0 |
| M3 | Editing basics | #4, #12, #13 | 10-12 days | 0.3.0 |
| M4 | Per-object and per-region settings | #10, #11, #14 | 15-20 days | 0.4.0 |
| M5 | Infill, supports, walls | #9, #15, #16 | 20-30 days | 0.5.0 |
| M6 | Output, hosts, profiles, platforms | #17, #18, #19, #21 | 10-14 days | 0.6.0 |
| M7 | Plates and sequential printing | #20 | 6-8 days | 0.7.0 |
| -- | Not planned | #22, #23 | -- | -- |

M1 and M2 give the largest quality gain for the cost. M3 can run alongside M1 or M2: it
touches the editor (`app.rs`, `gizmo.rs`, `geometry.rs`), not the slicer. M4 needs M2's
settings work (task M2.1). M5 is the most algorithm-heavy.

## M1: Print-quality core (0.1.0)

Goal: the same model sliced in PrintCraft and PrusaSlicer at matching settings shows no
obvious quality gap on the bridge and overhang tests.

| ID | Task | Done when | Est. |
|---|---|---|---|
| M1.1 | **Done.** **Per-feature speeds.** Add speeds for outer wall, inner wall, sparse infill, solid infill, top surface, support, bridge. They default to today's single print speed, so the output is unchanged. Use them in `gcode.rs` `print_path` by `PathKind`. | Test: each PathKind gets its feature's F value. Baseline unchanged with defaults. | 1.5 |
| M1.2 | **Done.** **Accelerations.** Add an optional acceleration per feature (outer wall, infill, travel, first layer). Emit `M204` (Marlin) or `SET_VELOCITY_LIMIT` (Klipper) only when the value changes. Include accelerations in the time estimate, replacing `ACCELERATION_ALLOWANCE`. | Test: commands appear only on change, in the right flavour. Estimate within 10 % of the sum of trapezoid times on the test projects. | 2 |
| M1.3 | **Done.** **Bridge detection.** In `slicer.rs`, find solid regions whose area below is empty (layer n-1 outline difference). Fill them with lines along the direction that crosses the shortest span. Add `PathKind::Bridge`. | Tests: a 20 mm gap between two pillars gives Bridge paths at the right angle, and a fully supported top does not. | 3 |
| M1.4 | **Done.** **Bridge settings.** Add bridge speed, flow ratio and fan %. Use the fan % on Bridge paths and restore the fan afterwards. Show a Bridge colour in the preview. | Test: fan and F change around the bridge paths. Screenshot of the bridge test. | 1 |
| M1.5 | **Done.** **Gap fill.** Fill the leftover space between the innermost wall and infill that is narrower than one line width with single lines (`PathKind::GapFill`). | Test: a 1.5 x line-width slot gets one gap-fill line, not a hole. | 2 |
| M1.6 | **Done.** **Avoid crossing walls.** Route a travel inside the current layer's outline when the straight line would cross a wall. If the inside route is over 2x longer, retract and go straight. | Test: travel between two points in a C-shape stays inside. Retraction count drops on the ORBIT fixture. | 2 |
| M1.7 | **Done.** **Calibration knobs.** Add a flow ratio (filament), an infill/wall overlap %, elephant-foot compensation (first layer inset) and XY hole compensation. | Tests: extrusion scales with the flow ratio, and the first-layer outline is inset by the set amount. | 1.5 |

## M2: Control and customisation (0.2.0)

Goal: any Marlin or Klipper printer can be driven by PrintCraft without editing G-code
afterwards.

| ID | Task | Done when | Est. |
|---|---|---|---|
| M2.1 | **Done.** **Settings registry.** One table describing every print setting: key, label, group, unit, range, default, simple or advanced. The settings panel and the project save/load are generated from it, not hand-written per field. This is a prerequisite for M4. | Every existing setting round-trips through a project from the registry. The panel looks the same (screenshot). | 3 |
| M2.2 | **Done.** **Custom start/end G-code.** Add start, end, before-layer-change and tool-change templates in the printer editor. The default templates reproduce today's output exactly. | Baseline unchanged with default templates. A user template shows up in the output. | 1.5 |
| M2.3 | **Done.** **Placeholders.** `{bed_temp}`, `{nozzle_temp}`, `{layer_num}`, `{layer_z}`, `{first_layer_temp}`, `{print_min_x}` etc., plus `{if}`/`{else}` on numeric comparisons. Unknown names give a slice error naming the placeholder. | Tests: each placeholder substitutes. A bad name gives the error, not silent text. | 2 |
| M2.4 | **Done.** **Colour change and pause at layer.** In the preview layer slider, add "Add colour change" (M600) and "Add pause" (M601/`PAUSE`) markers. They are stored in the project and shown on the slider. | Test: the command is emitted before the chosen layer. Screenshot of the markers. | 1.5 |
| M2.5 | **Done.** **Preview colouring.** Colour by speed, line width, layer height, fan and temperature, with a legend. Show time per feature in the legend. | Screenshots of each mode on the bridge test. The per-feature times add up to the total. | 2 |
| M2.6 | **Done.** **M73 progress.** Emit M73 P/R (remaining minutes) at each layer for Marlin/Prusa flavours. | Test: P rises monotonically to 100. | 0.5 |

## M3: Editing basics (0.3.0)

Goal: a model can be fixed, cut and placed without another tool.

| ID | Task | Done when | Est. |
|---|---|---|---|
| M3.1 | **Done.** **Undo/redo.** A command stack over plate edits: add, delete, move, rotate, scale, arrange, settings change. Ctrl+Z, Ctrl+Y and Ctrl+Shift+Z, with up to 100 steps, plus Edit menu entries. | Tests: each edit undoes to an equal fingerprint and redoes. | 3 |
| M3.2 | **Done.** **Non-uniform scale.** Separate X/Y/Z scale fields, with a lock toggle (locked by default) and a per-axis handle option. | Test: a 2:1:1 scale gives the expected bounds. Projects save it. | 1 |
| M3.3 | **Done.** **Lay flat on a picked face.** In "Lay flat" mode, hovering highlights the coplanar face group under the cursor and a click puts it down. | Test: picking each face of a rotated cube lands it flat. Screenshot. | 2 |
| M3.4 | **Done.** **Mesh repair.** On import: merge duplicate vertices, fix inconsistent winding, close simple holes (fan fill), drop degenerate triangles. Report what was fixed in the status line. | Tests: a cube with a missing face and flipped normals slices to the same layers as a clean cube. | 2.5 |
| M3.5 | **Done.** **Planar cut.** Cut at a Z height (horizontal plane first), keep upper, lower or both parts, cap the cut faces, and lay the parts flat. | Test: cutting a cube at half height gives two closed meshes of half the volume. | 3 |
| M3.6 | **Done.** **Split to objects.** Split a mesh with disconnected shells into separate objects. | Test: a two-shell mesh becomes 2 objects. | 1 |

## M4: Per-object and per-region settings (0.4.0)

Goal: different parts of a print can use different settings. This needs M2.1.

| ID | Task | Done when | Est. |
|---|---|---|---|
| M4.1 | **Done.** **Settings overlay model.** An object holds a sparse set of overrides over the plate settings, from the M2.1 registry. The slicer takes the effective settings per object. | Tests: two objects with different infill slice differently. Projects save the overrides. | 3 |
| M4.2 | **Done.** **Per-object settings UI.** The object list shows an "override" badge. A panel adds or removes overrides for the selected object. | Screenshot. Removing an override restores the plate value. | 2 |
| M4.3 | **Done.** **Height-range overrides.** Overrides from Z1 to Z2 for an object, for example denser infill near the top. | Test: the infill changes exactly at the range's layers. | 2.5 |
| M4.4 | **Done** (box, cylinder and sphere; infill density and pattern). **Modifier volumes.** Add a box, cylinder or sphere (or an imported mesh) as a modifier. Regions of the object inside it use the modifier's overrides. The slicer splits each layer's region by the modifier outline. | Test: a box modifier with 100 % infill in a cube gives solid infill only inside the box. | 4 |
| M4.5 | **Done.** **Support blockers and enforcers.** Special modifier volumes that remove or force supports. | Tests: a blocker removes supports in its footprint, and an enforcer adds them under a shallow overhang. | 2 |
| M4.6 | **Done** (one profile for the whole plate, worked out from every part, rather than one per object; chosen 2026-11-18 to keep shared layers). **Variable layer height.** Adaptive heights from surface slope, with a quality slider and a manual smoothing tool. Stored per object. `layer_heights` becomes per-object. Two-nozzle tool changes and the prime tower follow the merged layer list. | Tests: a sphere gets thinner layers near the top. Total height preserved. Dual output still valid. | 5 |

## M5: Infill, supports and walls (0.5.0)

Goal: PrusaSlicer's most used pattern and support choices are available.

| ID | Task | Done when | Est. |
|---|---|---|---|
| M5.1 | **Done.** **Gyroid infill.** Scan-converted approximation of the gyroid surface per layer, clipped to the infill region. | Test: connected lines inside the region, and the pattern changes with Z. Screenshot. | 3 |
| M5.2 | **Done.** **Honeycomb, cubic, adaptive cubic.** Add these patterns. Adaptive cubic thins the infill away from the surfaces. | Tests per pattern: inside the region, density within 10 % of target. | 4 |
| M5.3 | **Done.** **Lightning infill.** Infill only where needed to support top surfaces. | Test: a cube with a top gets a tree under the top only. | 4 |
| M5.4 | **Done.** **Top/bottom patterns.** Monotonic and concentric for solid and top layers. | Tests and screenshot. | 1.5 |
| M5.5 | **Done.** **Support settings.** Interface layers, top/bottom Z gap, XY gap, pattern spacing, all advanced. | Tests: the gap layers are empty, and the interface layers are dense. | 2 |
| M5.6 | **Done.** **Tree supports.** Branches that grow down from overhang points, avoid the model and merge, with tip and branch diameters. | Tests: an overhang test gets supports that don't touch the model sides. Volume is less than grid supports. Screenshot. | 10 |
| M5.7 | **Done** (experimental setting, off by default until tested on prints; walls widen over spaces too thin for a centre line and run on into centre lines; coverage beats normal walls on the test shapes; some ORBIT tooth tips still keep a small gap). **Variable-width walls (Arachne-style).** Make the wall line width vary to fill thin features without gaps. Off by default until it has been tested on prints. | Tests: a tapered wedge gets continuous walls with widths in the allowed range. Baseline unchanged when off. | 10 |

## M6: Output, hosts, profiles and platforms (0.6.0)

| ID | Task | Done when | Est. |
|---|---|---|---|
| M6.1 | **Done.** **Object labels.** `; printing object` / `EXCLUDE_OBJECT_DEFINE` for Klipper, and M486 for Marlin/Prusa. Add a "cancel object" button on the printer page for Moonraker and PrusaLink. | Tests per flavour. Cancelling works against the fake Moonraker. | 2 |
| M6.2 | **Done.** **Arc fitting.** Replace runs of short segments that fit a circle within a tolerance with G2/G3. Off for X3G (not supported there). | Test: a cylinder's walls become arcs within 0.01 mm. Time estimate unchanged within 2 %. | 2 |
| M6.3 | **Binary G-code (.bgcode).** Write Prusa's binary format (metadata, thumbnails, G-code blocks with heatshrink/MeatPack) from the published specification. Offer it for PrusaLink printers. | Test: the file round-trips through a decoder written from the spec. Upload to the fake PrusaLink. | 3 |
| M6.4 | **OctoPrint.** API-key connection, upload, start, status, temperatures, pause/stop, camera (MJPEG). Discovery by address scan. | Tests against a fake OctoPrint server. | 3 |
| M6.5 | **Duet / RepRapFirmware.** HTTP (rr_ and `/machine` APIs): upload, start, status, pause/stop. | Tests against a fake Duet. | 2.5 |
| M6.6 | **Filament library and user presets.** Save named filament and process presets, import/export them as JSON, and show the built-in generic filaments (PLA, PETG, ABS, TPU, ASA). | Tests: save, reload, import, export round-trip. | 2 |
| M6.7 | **Windows and macOS builds.** CI build for both. Fix path, serial and file-dialog differences found. | CI green on all three systems. App starts and slices the cube on each. | 2 |

## M7: Plates and sequential printing (0.7.0)

| ID | Task | Done when | Est. |
|---|---|---|---|
| M7.1 | **Sequential printing.** Print one object at a time, with an extruder clearance radius and height in the printer profile. Collision check blocks slicing with a clear message. | Tests: two objects print in sequence, and too-close objects are rejected. | 3 |
| M7.2 | **Multiple plates.** A project holds several plates, each sliced and exported or sent separately. Tabs above the 3D view. | Tests: project round-trip with 3 plates. Screenshot. | 4 |

## Not planned (from PCGAP.md items 22-23)

* Multi-material from one nozzle (MMU), IDEX and tool changers: these need the hardware
  to test. Revisit only if someone with one of these printers asks for it.
* STEP import (needs an OpenCASCADE-sized dependency), text/SVG emboss, measure tool,
  SLA resin, translations. They are out of scope for a simple FFF slicer unless asked.

## Order and checkpoints

1. M1, then M2. Release 0.1.0 and 0.2.0 after each; test-print the bridge and overhang
   tests on a real printer before tagging.
2. M3 can run in parallel with M1/M2 (different files).
3. M4 after M2.1.
4. M5 and M6 in either order. M6.1 to M6.5 are independent of each other.
5. M7 last.

Total: about 80-105 working days for M1-M7.

Track progress by marking task IDs done in this file and listing them in CHANGELOG.md.
