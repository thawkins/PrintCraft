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
| Infill | 25 % | 25 % (person's request) | 10 % |
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
   overlaps the wall by about 65 % of a line width). Estimated. The 15 % is the
   **Infill/wall overlap** setting (Advanced, 0-50 %, gap plan M1.7).
5a. **Size compensation** (`slicer::compensate`, gap plan M1.7; Advanced, both 0 by
   default). **Hole compensation** (0-1 mm) moves every hole's outline that far into the
   material on every layer, so holes come out larger. **Elephant foot** (0-1 mm) moves
   every outline of the first layer that far into the material (outer loops shrink,
   holes grow), offsetting the squash that widens the first layer; it is skipped with a
   full raft, where the raft takes the squash. A loop too small to offset is left as it
   is.
5b. **Flow ratio** (filament, 50-150 %, default 100 %, gap plan M1.7). Every printed
   line's filament is multiplied by it (bridges, gap fill and ironing too, on top of
   their own flow); the purge lines are not.
6. **Top/bottom (solid) areas.** The model is drawn onto a 1 mm grid on every layer. A grid
   cell on layer *i* is solid if it is empty on any layer from *i − bottom_layers* to
   *i + top_layers*. Layers near the very bottom or top are solid everywhere.
7. **Infill lines.** Straight parallel lines at 45° on even layers and 135° on odd layers,
   on a fixed grid so they stack. Solid areas use a spacing of `line_width`. Sparse
   areas use a spacing of `line_width × 100 / infill %`.
7a. **Line order** (`order_open_paths`, person's request). Infill, ironing, support and
   vase-bottom lines are not printed row by row. The next line is always the one whose
   nearer end is closest to the nozzle, and it is printed starting from that end. So
   where a hole or recess splits the rows, one side is filled completely before the
   nozzle moves to the other side, instead of jumping over the gap on every row.
   Ordering starts where the walls ended. Each sparse-infill direction (Grid,
   Triangles) is ordered on its own. End points are looked up in 3 mm buckets, so this
   stays fast. Estimated.
8. **Supports.** A face needs support when it faces down more steeply than the overhang
   angle: −normal.z > sin(angle). A support column starts at least one layer (≥ 0.15 mm)
   under the overhang. It goes down until it reaches the plate or the model. It keeps a
   one-cell (1 mm) gap from the model. Lines run along X on every second 1 mm row, which
   gives about 20 % density.
   With "Only from the plate", a column is dropped if any model lies below it in that
   cell (`Grid::supports`, `first_model`).
9. **Skirt.** 2 loops around the convex hull of the first layer, 5 mm away. **Brim.** 8 loops
   outwards from each outer outline on the first layer.
9a. **Raft** (person's request; `raft_paths`, `raft_height`). There are 3 raft layers.
   * The bottom layer has lines along X, 1 mm apart, so it grips the bed. The two layers
     above are solid, at one line width spacing, the first along Y and the second along X.
   * Raft area is worked out on the 1 mm grid, so its edge steps in 1 mm.
   * Raft lines are tagged `;TYPE:SUPPORT` in the print file and grey in the preview.
   * **Adhesion "Raft".** The model is lifted by the raft's thickness: the first layer
     plus 2 normal layers. The raft covers the model's first layer and every support
     column that reaches the plate, plus 3 mm all round. The model's first layer is laid
     0.15 mm higher than its normal height (an air gap), so the raft peels off. The
     model's bottom layers stay solid.
   * **Raft under support** (Support section, on by default; person's request). With
     this on, supports on, and any adhesion except Raft, every support column that
     reaches the plate stands on its own raft. With it off, the support lines go all the
     way down to the plate. The setting is greyed out when Adhesion is Raft, because the
     full raft already covers the supports. The raft fills the
     column's cells plus 2 mm all round, on the bottom 3 layers. It keeps a 1-cell (1 mm)
     gap from the model on each of those layers. The model still sits directly on the
     plate. Columns that stand on the model get no raft.
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
12b. **Bridges** (`slicer::bridge_areas`, gap plan M1.3). On every layer above the model's
    first, a 1 mm cell that is model on this layer but not on the one below is
    unsupported. Each 4-connected group of such cells is tried at 12 directions (every
    15°). For each cell, a line in that direction is followed both ways out of the group:
    it is anchored when both ends land on a cell that is model on this layer and the one
    below. The direction with the most anchored lines wins; among equals, the one with the
    shorter lines (the shortest span). A group with no anchored line (a one-sided
    overhang) or under 4 cells stays normal solid infill. A bridge's cells are widened by
    2 cells onto the supported area so its lines are anchored, and that area is taken out
    of the solid and sparse infill. Bridge lines are solid (one line width apart), printed
    before the layer's other infill, blue in the preview, and marked `;TYPE:SKIN` (the
    Cura-style names printer screens read have no bridge type). They print at the
    **bridge speed** (Auto: 50 % of the print speed), with **bridge flow** (default
    95 % of a normal line, 50-150 %) and the **bridge fan** (default 100 %). The fan is
    switched with `M106 ... ; bridge fan` before a bridge and back to the filament's fan
    (`; part fan`) before the next line that isn't one, only when the two differ. Walls
    over air are not bridged.
12c. **Gap fill** (`slicer::gap_fill_paths`, gap plan M1.5). For each outer loop of a
    layer (holes are not checked), the walls are offset one by one. Where the next wall,
    or the infill inside the last wall, doesn't fit, the space left inside the innermost
    wall (or the whole shape, when not even the outer wall fits) is narrower than a wall.
    Its width is found by searching for the deepest offset of the shape that still
    exists, plus the thickness that almost-collapsed loop still has. One line is laid
    along that loop between its two farthest-apart points, as wide as the space (so it
    lays only the plastic the space holds; capped at 1.5 line widths). Spaces under 0.3
    of a line width stay empty. White in the preview, `;TYPE:WALL-INNER` (Cura-style
    names have no gap-fill type), printed at the print speed. Not in vase mode. Only
    strip-like spaces are filled well: a gap gets one line.
13. **Spiral vase.** One wall. The first `bottom_layers` layers are solid. After that there
    is only the outer wall: no infill, no top.
14. **Lay flat on a picked face** (`PlacedObject::lay_flat_on`, gap plan M3.3). With the
    Face tool (F) the mouse's ray is tested against every part's triangles
    (Moller-Trumbore); the nearest hit's triangle and the others in the same flat face
    (facing within about 1 degree, in the same plane within 0.05 mm) are drawn orange. A
    click turns the part so that face points down, by the same turn as below, and
    selects it. Faces are measured with the part's X/Y/Z scale applied.
    **Lay flat** (`PlacedObject::lay_flat`). Faces are grouped by direction (about 1°).
    The group with the most area is turned to face down: turn about X by atan2(nᵧ, n_z),
    then about Y by atan2(−nₓ, √(nᵧ²+n_z²)) + 180°. Spin about the vertical is kept.

15. **Mesh repair on import** (`src/repair.rs`, gap plan M3.4). Points within 0.0001 mm
    are merged (exact copies first, then a grid of 0.0001 mm cells; the first point seen
    is kept). A triangle with two corners on one point is removed. Triangles sharing an
    edge used by exactly two are turned so they run along it in opposite directions;
    in each part the facing most triangles had is kept. A loop of edges used by one
    triangle each is a hole and is closed with a fan from its first point, unless it
    passes a point twice or has as many edges as its part has triangles (the rim of an
    open sheet). A closed part with negative volume is turned inside out. With nothing
    to fix, the mesh is returned untouched.

16. **Planar cut** (`src/cut.rs`, gap plan M3.5). The part's placed triangles are split
    at the height (moved up 0.001 mm while a corner lies on it). A triangle crossing
    the plane is cut into one triangle on one side and two on the other; the cut points
    are computed with the edge's ends in a fixed order, so neighbours get identical
    points. Slivers whose cut is a single point add nothing to the cut. The cut edges
    are chained into loops; loops are outlines or holes by how many others they lie in.
    Each outline and its holes (joined by a bridge edge from each hole's rightmost point,
    after Eberly) is filled by ear clipping. The lower part gets the cap facing up, the
    upper part the same cap facing down, and is then turned half a turn about X.

## Print file and estimates (src/gcode.rs)

* **Extrusion per move.** `length × line_width × layer_height ÷ (π × (filament ⌀ / 2)²)`.
  This assumes a rectangular line shape, so it is a simplification. Absolute extrusion
  (M82) is used.
* **Retraction.** Before any travel longer than 2 mm, retract by the printer's retraction
  length, then un-retract before printing.
* **Avoid crossing walls** (`travel.rs`, gap plan M1.6; Advanced, off by default, as in
  PrusaSlicer). The layer's outer wall loops, widened by 0.45 line width to the part's
  surface, are the boundaries. A travel that leaves the part through a loop and comes
  back through the same loop (across a slot, round a concave bay, over a hole) instead
  walks along that outer wall loop, the shorter way round, between the points nearest
  where it went out and came back in. Such a travel stays inside and is **not
  retracted** (and gets no z-hop or wipe). A travel that starts or ends outside the part,
  goes to another island, or whose route is more than **2×** the straight distance, is
  made as before: straight, retracted when over 2 mm. On the ORBIT fixture this cuts the
  retraction lines from 18,208 to 6,720.
* **Speeds.** The first layer prints at the first-layer speed. After that, each kind of
  line has its own speed (Advanced: outer wall, inner wall, sparse infill, solid infill,
  top surface, support, bridge). Each starts on "Auto": outer walls and bridges at 50 %
  of the print speed, the others at the print speed. A speed set by hand (15-500 mm/s) replaces Auto and
  stays when a quality preset is chosen. Skirt, brim, raft, prime tower and ooze shield
  print at the print speed; ironing at 15 mm/s.
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
* **Progress** (gap plan M2.6). On Marlin and Prusa firmware printers, each layer gets
  `M73 P<percent> R<minutes left>` (after its `;LAYER:` marker and any custom
  before-each-layer block), and the end gets `M73 P100 R0`. Percent is by time (the
  estimate so far over the total), rounded down so only the end says 100; minutes are
  rounded up, so only the end says 0. Klipper printers work out their own progress and
  get none; X3G files carry their own, and the translator skips `M73`.
* **Preview colours and times** (gap plan M2.5). The G-code writer records, for every
  path, what it is printed at: speed after cooling slow-down, line width, part fan,
  nozzle temperature and the seconds it takes (`GcodeOutput::path_stats`), and splits
  the print time into each kind of line, travel, and everything else (heating, purge,
  retractions, layer and tool changes), which add up exactly to the estimate
  (`GcodeOutput::times`). The Show box's "Colour by" picks line type, nozzle (two-nozzle
  printers, not Ditto), speed, line width, layer height, fan or temperature. The value
  modes colour each path from blue (lowest) through cyan, green and yellow to red
  (highest), scaled to the lowest and highest value in the whole print, with a scale
  bar showing the lowest, middle and highest value. The time beside each line type,
  and the Travel and "Heating, purge, changes" rows, are those times.
* **Colour change and pause at a layer** (`src/layer_events.rs`, gap plan M2.4). In the
  preview, "+ Colour change here" / "+ Pause here" add one before the layer the slider
  shows (not the first layer; not on MakerBot / Sailfish printers, which have neither
  command). Each is stored by its height and saved in the project; it goes before the
  first layer at or above that height, so it stays put if the layer height changes. The
  command goes right after that layer's `;LAYER:` marker, before the move up: `M600`
  for a colour change (Klipper set-ups need an `M600` macro), and `PAUSE` on Klipper or
  `M601` elsewhere for a pause. They are added to the finished G-code text, so adding
  or removing one needs no new slice. The preview marks them beside the layer slider
  (orange: colour change, blue: pause) and lists them with a remove button.
* **Custom G-code** (printer editor > Custom G-code, gap plan M2.2). Four blocks per
  printer, all empty by default, which leaves the output unchanged.
  * **Placeholders** (`src/gcode_template.rs`, gap plan M2.3): `{name}` is replaced by
    a value (whole numbers without decimals, others to at most 3), `{name-1}` /
    `{name+5}` by the value plus or minus a number, and `{if a op b}` ... `{else}` ...
    `{endif}` keeps one part (`op`: `== != < <= > >=`; `a`, `b`: names or numbers;
    `{if name}` is true when it isn't 0; branches nest). `{{` and `}}` write braces.
    The names (`gcode::PLACEHOLDERS`, also listed by hovering in the editor): bed_temp,
    nozzle_temp, first_layer_temp, nozzle_temp_0, nozzle_temp_1, fan_percent,
    filament_diameter, nozzle_diameter, retract_length, extruders, klipper (1/0),
    bed_x, bed_y, max_z, layer_height, first_layer_height, total_layers, layer_num,
    layer_z, print_min_x/max_x/min_y/max_y (printer coordinates of everything
    printed, skirt included), print_max_z, tool, next_tool. Layer values are the
    first layer's in Start and the last layer's in End; at a tool change `tool` is
    the old nozzle and `next_tool` the new one.
  * **Checked before slicing:** an unknown name (even in a part an `{if}` skips) or a
    malformed tag stops the slice with a message naming the box and the tag, e.g.
    "Custom G-code (End): unknown placeholder {nozle_temp}"; the job can't be sent or
    exported until it's fixed.
  * **Start** replaces heating, homing and the purge lines (on a two-nozzle printer,
    also the second nozzle's purge, parking and standby). The header comments come
    before it. After it PrintCraft writes `G90`, `M82`, `G92 E0` and, with two nozzles,
    selects the first one (`T0`/`T1`, `G92 E0`), so its own moves are right whatever
    the start did. The nozzle is assumed to start at the printer's origin.
  * **End** replaces everything after `;END` (retract, lift, park, heaters and motors
    off).
  * **Before each layer** is added right after each `;LAYER:` marker, before the move
    up (and any layer-change retraction).
  * **Tool change** is added at each tool change, just before the new nozzle is
    selected (after the old one is set aside).
  * X3G printers: custom code is translated like the rest; G-codes the translator
    doesn't know stop the export with an error naming the line.
  * **Insert default** (a button beside each box) adds PrintCraft's own text for that
    place to the box, to edit; nothing in the box is replaced.
    * Start: the built-in start for this printer on its primary nozzle
      (`gcode::default_start_gcode`, the same code the built-in start runs), with
      temperatures as `{bed_temp}`, `{nozzle_temp}` and, for Klipper's waits,
      `{bed_temp-1}` / `{nozzle_temp-1}`. Filled in, it is exactly the built-in start.
    * End: the built-in end's steps written without knowing the print: heaters and fan
      off first, then a relative 2 mm retract and 10 mm lift (`G91` ... `G90`), park at
      the back-left, motors off. Heaters go off before the lift, so they are off even if
      the lift is refused on a print that reaches the printer's top.
    * Before each layer and Tool change: comment lines describing what PrintCraft
      writes there itself (with this printer's tool-change retraction and standby
      drop); comments change nothing.
* **Coordinates.** The app centres everything on the plate. The print file adds half the
  plate size, so (0, 0) is the printer's front-left corner.
* **Time estimate.** Each printed line and each travel is timed with the printer's
  acceleration (`motion.rs`): it starts and ends at rest, speeds up and slows down at the
  acceleration in use, and takes corners at a speed set by the corner's angle (Klipper's
  square-corner rule, 5 mm/s through a 90° corner; straight joins are not slowed). Then
  + retraction time + 1 s per layer + 120 s warm-up. Jerk and firmware buffer limits are
  not modelled. Estimated. (Before M1.2: length ÷ speed × 1.15.)
* **Accelerations.** The printer editor has the printer's own **acceleration** (mm/s²,
  100-30000): Ender-3 V3 KE 8000; Prusa MK4S, Core One and XL 4000; MK3S+ and MINI+
  1250; generic printers and the Creator Pro 1000 (estimated from the makers' firmware
  defaults). Advanced print settings add outer wall, infill (sparse and solid), travel
  and first layer accelerations, each "Auto" (the printer's own) until set. While all
  are on Auto nothing is written. Once one is set, a change is written before the moves
  that need it, and only when the value changes: `M204 P` (printing) and `M204 T`
  (travel) for Marlin and Prusa firmware, `SET_VELOCITY_LIMIT ACCEL=` for Klipper (one
  acceleration for every move). X3G printers get none (Sailfish has no such command).
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
