# PrintCraft mechanics sheet

## Where these numbers come from

This project is a **clean-room** rebuild. The project's `AGENTS.md` forbids inspecting,
extracting or decompiling the original AppImage. So no function addresses from the original
are cited, and nothing here was read from its code. `reference/` was empty.

Every value below is a common default from general 3D-printing practice, so each one is
marked **estimated**. The "Source" column names the place in *this* app's code where the
value lives. If screenshots or exported profiles of the original are added to `reference/`,
these values can be matched to it.

## Printers (src/profiles.rs `Printer::presets`)

| Preset | Plate (mm) | Max height | Nozzle | Retraction | Status |
|---|---|---|---|---|---|
| Compact printer 220 × 220 (default) | 220 × 220 | 250 | 0.4 | 0.8 mm @ 40 mm/s | estimated |
| Standard printer 235 × 235 | 235 × 235 | 250 | 0.4 | 0.8 mm | estimated |
| Large printer 300 × 300 | 300 × 300 | 400 | 0.4 | 1.0 mm | estimated |
| Mini printer 180 × 180 | 180 × 180 | 180 | 0.4 | 0.8 mm | estimated |
| Prusa MK4S | 250 × 210 | 220 | 0.4 | 0.7 mm | size: maker's published spec; retraction estimated |
| Prusa Core One | 250 × 220 | 270 | 0.4 | 0.7 mm | size: published spec; retraction estimated |
| Prusa MK3S+ | 250 × 210 | 210 | 0.4 | 0.8 mm | size: published spec; retraction estimated |
| Prusa MINI+ | 180 × 180 | 180 | 0.4 | 3.2 mm (Bowden) | size: published spec; retraction estimated |
| Prusa XL (one tool) | 360 × 360 | 360 | 0.4 | 0.7 mm | size: published spec; retraction estimated |
| Creality Ender-3 V3 KE | 220 × 220 | 240 | 0.4 | 0.8 mm | size: published spec; retraction estimated |

## Filaments (src/profiles.rs `Filament::presets`)

All are 1.75 mm filament.

| Filament | Nozzle °C | Plate °C | Density g/cm³ | Fan % | Price / kg | Status |
|---|---|---|---|---|---|---|
| PLA (default) | 210 | 60 | 1.24 | 100 | 20 | estimated |
| PETG | 240 | 80 | 1.27 | 50 | 22 | estimated |
| ABS | 250 | 100 | 1.04 | 0 | 22 | estimated |
| TPU | 225 | 50 | 1.21 | 60 | 30 | estimated |

## Quality presets (src/profiles.rs `Quality::settings`)

| | Fine | Standard (default) | Draft |
|---|---|---|---|
| Layer height | 0.12 | 0.20 | 0.28 |
| First layer | 0.20 | 0.20 | 0.28 |
| Walls | 3 | 2 | 2 |
| Top / bottom layers | 6 / 5 | 4 / 3 | 3 / 3 |
| Infill | 15 % | 15 % | 10 % |
| Print speed (mm/s) | 50 | 60 | 80 |

These apply to all three presets: first-layer speed 25 mm/s, travel 150 mm/s, support
overhang angle 50° from vertical, supports off, adhesion = skirt. All are estimated.

The defaults for the newer options are:
* infill pattern Grid
* seam Aligned
* ironing, fuzzy skin and vase mode off
* z-hop 0 mm
* minimum layer time 15 s
* supports may stand on the model

All are estimated. Choosing a quality preset changes only the values in the table above
(`Quality::apply`).

## Slicing rules (src/slicer.rs)

1. **Layers.** The first layer is `first_layer_height` thick. After that, layers are added
   every `layer_height` until the model's top (`layer_heights`). Each layer is cut at its
   middle height.
2. **Outlines.** Triangle–plane cuts are chained into closed loops (points are snapped to
   0.001 mm). Outer loops run counter-clockwise and holes run clockwise, decided by how
   deeply each loop is nested (`layer_outline`).
3. **Line width** = nozzle × 1.05 (0.42 mm for a 0.4 mm nozzle). `pipeline.rs line_width`. Estimated.
4. **Walls.** Wall *k* (0 = outside) is the outline moved inwards by `line_width × (k + 0.5)`.
   Walls print from the inside out, with the outer wall last. Loops that would collapse are
   dropped (`offset_left`).
5. **Infill area.** The outline moved inwards by `line_width × (walls − 0.15)`, so infill
   lines end 15 % of a line width inside the innermost wall's band (the infill bead itself
   overlaps the wall by about 65 % of a line width). Estimated.
6. **Top/bottom (solid) areas.** The model is drawn onto a 1 mm grid on every layer. A grid
   cell on layer *i* is solid if it is empty on any layer from *i − bottom_layers* to
   *i + top_layers*. Layers near the very bottom or top are solid everywhere.
7. **Infill lines.** Straight parallel lines at 45° on even layers and 135° on odd layers,
   on a fixed grid so they stack. Solid areas use a spacing of `line_width`. Sparse
   areas use a spacing of `line_width × 100 / infill %`.
8. **Supports.** A face needs support when it faces down more steeply than the overhang
   angle: −normal.z > sin(angle). A support column starts at least one layer (≥ 0.15 mm)
   under the overhang. It goes down until it reaches the plate or the model. It keeps a
   one-cell (1 mm) gap from the model. Lines run along X on every second 1 mm row, which
   gives about 20 % density.
   With "Only from the plate", a column is dropped if any model lies below it in that
   cell (`Grid::supports`, `first_model`).
9. **Skirt.** 2 loops around the convex hull of the first layer, 5 mm away. **Brim.** 8 loops
   outwards from each outer outline on the first layer.
10. **Infill patterns** (`infill_angles`). With base spacing `s = line_width × 100 / infill %`:
    * **Lines:** one set at 45°/135°, alternating each layer, spacing *s*.
    * **Grid:** 45° and 135° on every layer, spacing 2*s*.
    * **Triangles:** 0°, 60° and 120°, spacing 3*s*.

    The density stays the same in all three.
11. **Ironing.** Only on cells that are model on this layer and not on the next one, so
    only the topmost surface layer is ironed. The lines are 0.2 mm apart and at right
    angles to that layer's solid infill.
12. **Fuzzy skin** (`fuzz`). The outer wall gets a point every 0.8 mm, each pushed in or
    out by up to 0.3 mm (repeatable pseudo-random). The first layer is skipped so the
    print sits flat.
12a. **Top surface and Fuzzy top** (person's request). Solid lines on the visible top
    surface are marked as their own kind, **Top surface** (pink in the preview,
    `;TYPE:TOP-SURFACE`). A cell counts as top surface when it is model on this layer and
    not on the next. With **Fuzzy top** on (off by default), each top-surface line is cut
    into steps of at most 0.5 mm. Each step's move carries a Z of layer top + r × amount,
    where r is a repeatable pseudo-random number from 0 to 1 (`gcode::fuzzy_unit`). The
    amount defaults to **0.1 mm** and can be set from 0.02 to 0.3 mm. The nozzle only lifts,
    never dips into the layer below. Plastic per millimetre is unchanged. After each line
    the nozzle goes back to the layer height (`G1 Z<layer> F600`). Only the visible top
    layer is textured, because the top layers under it are covered. Ironing is skipped
    while Fuzzy top is on, since it would flatten the texture.
13. **Spiral vase.** One wall. The first `bottom_layers` layers are solid. After that there
    is only the outer wall: no infill, no top.
14. **Lay flat** (`PlacedObject::lay_flat`). Faces are grouped by direction (about 1°).
    The group with the most area is turned to face down: turn about X by atan2(nᵧ, n_z),
    then about Y by atan2(−nₓ, √(nᵧ²+n_z²)) + 180°. Spin about the vertical is kept.

## Print file and estimates (src/gcode.rs)

* **Extrusion per move.** `length × line_width × layer_height ÷ (π × (filament ⌀ / 2)²)`.
  This assumes a rectangular line shape, so it is a simplification. Absolute extrusion
  (M82) is used.
* **Retraction.** Before any travel longer than 2 mm, retract by the printer's retraction
  length, then un-retract before printing.
* **Speeds.** The first layer prints at the first-layer speed. Outer walls print at 50 %
  of the print speed. Everything else prints at the print speed.
* **Fan.** Off on layer 1, then the filament's fan % from layer 2 onwards.
* **Heat waits (src/heat.rs) — only while heating up, never for cooling** (person's
  request). A nozzle or plate already hotter than the job needs is set to the new target,
  and the print carries on without waiting.
  * **Klipper printers** (the KE preset, or "Runs Klipper firmware" on; also every file
    sent to a Klipper printer): `M140 S<t>` + `TEMPERATURE_WAIT SENSOR=heater_bed
    MINIMUM=<t−1>`, and `M104 S<t>` + `TEMPERATURE_WAIT SENSOR=extruder MINIMUM=<t−1>`.
    Klipper's own `M109`/`M190` wait in both directions, so they are not used.
  * **Marlin and Prusa firmware:** `M190 S<t>` / `M109 S<t>`, which already wait only
    while heating. The `R` form, which also waits for cooling, is never written.
  * The calibration step's "back to job temperature" wait follows the same rule.
* **Start.** Heat the plate and nozzle, home, and wait for both to heat up. Then purge
  (`purge_lines`) to clear the nozzle:
  * lift, move to X 1.5, Y 20, then drop to Z 0.3
  * push 2 mm of filament in place (to refill what oozed while heating)
  * draw line 1 up the left edge to Y = 20 + length
  * step 0.4 mm right and draw line 2 back down
  * lift 2 mm

  Length = plate depth − 40 mm, at most 200 mm (180 mm on a 220 mm plate). The two
  lines share the printer's **purge amount** (default **30 mm** of filament, editable in
  the Printer box, estimated from common start routines for this size of printer). They
  print at 25 mm/s. The person reported the old single 9 mm line didn't clear the nozzle;
  the new purge pushes about 3× as much.
* **End.** Retract 2 mm, lift 10 mm, park at the back-left, heaters off, motors off.
* **Coordinates.** The app centres everything on the plate. The print file adds half the
  plate size, so (0, 0) is the printer's front-left corner.
* **Time estimate.** Sum of (move length ÷ speed) + travel time + retraction time + 1 s per
  layer + 120 s warm-up, then × 1.15 to allow for acceleration. Estimated.
* **Weight.** Filament volume × density. **Cost** = weight × price per kg.
* **Seam** (`seam_start_aligned`). Each loop starts at a point chosen by the seam setting:
  * **Aligned (default, as shown in reference/picture-2.png):** the point closest to
    where the wall loops of the layer below started, so the seam forms one line. It
    starts at the back on the first layer.
  * **Nearest:** the point closest to the nozzle.
  * **Rear:** the point with the largest Y (the back of the plate); ties go to the point
    nearest the middle.
  * **Random:** a repeatable hash of the layer and path. Vase mode always uses Nearest.
* **Z-hop.** After a retraction, the nozzle lifts by z-hop mm (F600) for the travel move,
  then drops back before un-retracting.
* **Ironing.** 15 % of normal flow at 15 mm/s.
* **Cooling slowdown.** Layer time = Σ(path length ÷ speed). If it is below the minimum
  layer time, every printing speed on that layer is multiplied by (time ÷ minimum).
* **Minimum printing speed.** No printing move goes slower than 15 mm/s
  (`gcode.rs MIN_PRINT_SPEED`): not the cooling slowdown, and not a speed set in the
  settings (the panel starts its speeds at 15 mm/s; a project file asking for less is
  raised to 15). Ironing keeps its own 15 mm/s. Decided 2026-11-17.
* **Spiral vase G-code.** In spiral layers there is no Z move at the layer start. Each
  outer-wall move carries Z rising from (layer top − layer height) to the layer top, in
  step with the distance printed.
* **Layer count.** The header has `;LAYER_COUNT:N` and `; total layer number: N`. Files sent
  to Klipper printers also get `SET_PRINT_STATS_INFO TOTAL_LAYER=N` / `CURRENT_LAYER=k`
  (src/printer/layer_info.rs).

## Controls (src/app.rs, src/view3d.rs)

* Left-drag on a model moves it on the plate. Left-drag on empty space turns the view.
  Right- or middle-drag pans. The scroll wheel zooms.
* Click selects a model. Delete removes it. Ctrl+O opens files. You can also drop STL, OBJ
  or 3MF files onto the window.
* A selected model shows selection marks (src/gizmo.rs): corner brackets, its size in mm, and
  the handles of the current tool. M, S and R, or the strip on the left of the view, choose
  Move, Scale or Rotate. Handles grab within 9 px of the pointer and never shrink below 90 px
  (arrows) or a 75 px ring radius on screen.
  * Move arrows change X or Y only. They reach half the model's largest side plus 12 mm.
  * Scale corners scale all three sides together, by the change in on-screen distance from the
    model's centre. The result is 1-2000 %, in whole percent.
  * Rotate rings add to the model's X, Y or Z angle. The angle is kept within -180..180° and
    rounded to 0.1°. The ring radius is half the box diagonal plus 6 mm. After any handle drag
    the model drops back onto the plate.
* In Preview, the vertical slider or the Up/Down keys step through layers.
* Models outside the plate turn orange, and slicing is refused until they fit.
* Adding models re-arranges the plate into rows with a 6 mm gap.

## Left out to keep it simple (compared with a full slicer)

* Multi-colour and multi-material printing, painting supports or seams, and per-object
  settings.
* Tree supports, adaptive layers, arc moves, Arachne walls, more infill patterns, and cloud
  services. See FEATURES.md for the full comparison with PrusaSlicer.
* STEP/AMF import and binary G-code.
