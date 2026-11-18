# PrintCraft gap analysis: PrusaSlicer and Simplify3D

Date: 2026-11-18. PrintCraft version: 0.0.5.

## Sources and limits

* **PrintCraft:** read from the source in `src/` (about 23,400 lines), `FEATURES.md`,
  `MECHANICS.md` and `PRINTER_LINK.md` at version 0.0.5.
* **PrusaSlicer (2.9 series) and Simplify3D (5.x):** from their widely documented
  feature sets (public knowledge). The vendors' pages were not checked for this
  document. A feature added or removed in a newer release may be listed wrongly here.
  Check any item against the vendor's documentation before acting on it.
* PrintCraft aims to be a *simpler* slicer. A gap is listed whether or not it should be
  closed. Section 4 ranks which gaps matter. `PCGAPPLAN.md` turns the ranking into work.

Key:

| Mark | Meaning |
|---|---|
| Yes | Does it. |
| Part | A smaller or fixed version of the same idea. |
| No | Missing. |
| n/a | Not applicable to that product. |

## 1. Summary

PrintCraft already covers a single plate of FFF prints: basic walls, infill and supports,
G-code export, and sending to several printer types. In two areas it does more than
PrusaSlicer:

* printer connectivity: Creality network, discovery, cameras, USB streaming;
* old MakerBot/Sailfish machines: X3G and two nozzles on one carriage.

The largest gaps, in order of how much they affect print quality or daily use:

1. **Slicing quality.** There is no bridge detection, no per-feature speeds, no
   acceleration control, no gap fill, no Arachne walls, and no overhang handling. This
   is the area where prints will look worse than PrusaSlicer's.
2. **Per-object and per-region control.** There are no per-object settings, modifiers,
   height-range settings, variable layer height, or support enforcers/blockers.
3. **Editing basics.** There is no undo/redo, cut tool, mesh repair, or pick-a-face lay-flat.
4. **Customisation.** There is no custom start/end G-code, no placeholders, no
   post-processing scripts, and no colour change or pause at a layer.
5. **Infill and supports.** There are only 3 infill patterns and no tree supports.
6. **Output formats and hosts.** There is no binary G-code, arc fitting, object labels,
   OctoPrint or Duet.
7. **Multiple plates, sequential printing, multi-material (MMU),** and SLA.

## 2. Feature comparison

### 2.1 Model import and preparation

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| STL, OBJ, 3MF import | Yes | Yes | Yes |
| STEP import | No | Yes | No |
| AMF import | No | Yes | Yes |
| Move, rotate, scale, duplicate | Yes | Yes | Yes |
| Auto-arrange | Yes | Yes (with rotation) | Yes |
| Lay flat | Part (biggest face only) | Yes (pick a face) | Yes (pick a face) |
| Non-uniform scale (X/Y/Z separately) | No | Yes | Yes |
| Undo / redo | No | Yes | Yes |
| Mesh repair (holes, normals, non-manifold) | No | Yes | Yes |
| Mesh simplify (decimate) | No | Yes | No |
| Cut tool (planar, with connectors) | No | Yes | Part (cross-section only) |
| Split into objects / parts | No | Yes | Yes |
| Merge models into one object | No | Yes | Yes |
| Text and SVG emboss | No | Yes | No |
| Measure tool | No | Yes | No |
| Built-in shape gallery | No | Yes | No |
| Multiple plates (beds) | No | Yes (2.9) | No |
| Sequential (one-at-a-time) printing with collision check | No | Yes | Yes |

### 2.2 Per-object and per-region settings

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Per-object settings | No (only the extruder, per object) | Yes | Yes (process per object) |
| Modifier meshes (change settings inside a volume) | No | Yes | Part (via processes) |
| Negative volumes (subtract) | No | Yes | No |
| Settings by height range | No | Yes | Yes (multiple processes) |
| Variable layer height (adaptive and painted) | No | Yes | Part (by height range) |
| Support enforcers and blockers | No | Yes (mesh and paint) | Yes (manual support placement) |
| Seam painting | No | Yes | No |
| Multi-material painting | No | Yes | No |

### 2.3 Walls, surfaces and seams

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Walls, top and bottom layers | Yes | Yes | Yes |
| Variable-width walls (Arachne) | No | Yes | No |
| Gap fill between walls | No | Yes | Yes |
| Wall order (inner/outer first) | No (fixed) | Yes | Yes |
| Extra walls on overhangs, overhang reversal | No | Yes | No |
| Ensure vertical shell thickness | No | Yes | No |
| Seam position (nearest, rear, random) | Yes | Yes | Yes |
| Scarf seam | No | Yes | No |
| Ironing | Part (fixed values) | Yes | No |
| Fuzzy skin | Part (fixed values) | Yes | No |
| Spiral vase | Yes | Yes | Yes |
| Hole/horizontal size compensation | No | Yes | Yes |
| Elephant-foot compensation | No | Yes | No |

### 2.4 Infill

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Infill patterns | Part (lines, grid, triangles) | Yes (about 15) | Yes (about 8) |
| Gyroid | No | Yes | Yes |
| Honeycomb, cubic, adaptive cubic, lightning | No | Yes | Part (honeycomb, full honeycomb) |
| Top/bottom pattern choice (concentric, monotonic) | No | Yes | Yes |
| Infill every N layers / combine infill | No | Yes | Yes |
| Infill/wall overlap setting | No | Yes | Yes |
| Solid infill threshold area | No | Yes | No |

### 2.5 Supports and adhesion

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Grid supports with overhang angle | Yes | Yes | Yes |
| Build plate only | Yes | Yes | Yes |
| Tree / organic supports | No | Yes | No |
| Manual support add/remove | No | Yes (enforcers/blockers) | Yes (click to add/remove pillars) |
| Support interface layers, Z gap settings | No (fixed) | Yes | Yes |
| Dense support / separate support extruder | Part (support nozzle on 2-nozzle printers) | Yes | Yes |
| Skirt, brim | Yes | Yes | Yes |
| Brim type (outer/inner/ears) | No | Yes | No |
| Raft | Yes | Yes | Yes |
| Prime pillar / tower | Yes (2 nozzles) | Yes (wipe tower) | Yes (prime pillar) |
| Ooze shield | Yes | No | Yes |

### 2.6 Speed, motion, cooling, extrusion

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Per-feature speeds (outer wall, infill, support, bridges) | No (one print speed, first layer, travel) | Yes | Yes (by percentage) |
| Acceleration and jerk control per feature | No | Yes | No |
| Max volumetric speed / flow limit | No | Yes | No |
| Dynamic overhang speed | No | Yes | No |
| Bridge detection and bridge settings (speed, flow, fan) | No | Yes | Yes |
| Avoid crossing walls on travel | No | Yes | Yes |
| Retraction, wipe, Z-hop | Yes | Yes | Yes |
| Coast at end of path | No | No | Yes |
| Firmware retraction (G10/G11) | No | Yes | Yes |
| Minimum layer time slowdown | Yes | Yes | Yes |
| Fan control by layer / feature | Part (on after first layer) | Yes | Yes |
| Flow ratio / extrusion multiplier | No | Yes | Yes |
| Pressure / linear advance setting | No | Part (in G-code) | No |
| Temperature by layer (tower) | No | Yes (custom G-code) | Yes (temperature by layer) |

### 2.7 G-code and output

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Plain G-code export | Yes | Yes | Yes |
| X3G export (MakerBot/Sailfish) | Yes | No | Yes |
| Binary G-code (.bgcode) | No | Yes | No |
| Arc fitting (G2/G3) | No | Yes | No |
| Custom start/end G-code | No (fixed) | Yes | Yes |
| G-code placeholders / macros (variables, conditions) | No | Yes | Yes |
| Per-layer or tool-change scripts | No | Yes | Yes |
| Post-processing scripts | No | Yes | Yes (commands) |
| Colour change, pause, custom code at a layer | No | Yes (preview slider) | Yes |
| Object labels for cancel (EXCLUDE_OBJECT, M486) | No | Yes | No |
| Thumbnails in G-code | Part (Creality format) | Yes (several formats) | No |
| Progress (M73) | No | Yes | No |
| G-code flavours (Marlin, Klipper, RepRap, Sailfish) | Part (Marlin/Klipper, X3G) | Yes | Yes |

### 2.8 Preview and analysis

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Layer slider, colour by feature | Yes | Yes | Yes |
| Colour by nozzle | Yes | Yes | Yes |
| Colour by speed, flow, width, height, fan, temperature | No | Yes | Yes |
| Move-by-move (horizontal) slider | No | Yes | Yes (animation) |
| Time, filament, cost estimate | Yes | Yes | Yes |
| Time per feature / per layer | No | Yes | Yes |
| Open an external G-code file in the viewer | No | Yes | Yes |
| Cross-section view | No | Yes (clipping plane) | Yes |

### 2.9 Profiles and configuration

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Printer presets | Yes (10) | Yes (hundreds, vendor bundles) | Yes (hundreds) |
| Filament library | Part (few built-in) | Yes | Yes |
| User presets saved and named | Part (printers only) | Yes | Yes |
| Preset inheritance / compare presets | No | Yes | No |
| Import/export settings bundles | No | Yes | Yes (FFF profiles) |
| Configuration snapshots / rollback | No | Yes | No |
| Expert vs simple mode | n/a (always simple) | Yes | Yes |
| Settings search | No | Yes | No |
| Calibration prints (temp, flow, retraction) | Part (printer bed calibration) | Part (Prusa wizard) | No |

### 2.10 Printers and connectivity

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Creality network service | Yes | No | No |
| Moonraker / Klipper | Yes | Part (upload only) | No |
| PrusaLink | Yes | Yes | No |
| Prusa Connect (cloud) | No | Yes | No |
| OctoPrint | No | Yes | No |
| Duet / RepRapFirmware | No | Yes | No |
| Network discovery | Yes | Part (Bonjour) | No |
| Live monitoring, pause/stop | Yes | Part (Connect) | Yes (USB control panel) |
| Camera view | Yes | No | No |
| USB printing (Marlin) | Yes | No | Yes |
| MakerBot / Sailfish over USB | Yes | No | Yes |
| Two nozzles on one carriage, Ditto | Yes | No (tool changers/MMU instead) | Yes |
| Multi-material (single nozzle, MMU) | No | Yes | Part |
| Tool changer / IDEX | No | Yes (XL) | Yes (IDEX, mirror/duplicate) |
| SLA resin | No | Yes | No |

### 2.11 Platform and usability

| Feature | PrintCraft | PrusaSlicer | Simplify3D |
|---|---|---|---|
| Linux | Yes | Yes | Yes |
| Windows, macOS | Unverified (not built or tested) | Yes | Yes |
| Translations | No (English only) | Yes | Part |
| Projects with plate, settings and printer | Yes | Yes (3MF) | Yes (factory files) |
| Save project as 3MF (interchange) | No | Yes | No |
| Command-line slicing | No | Yes | No |
| Keyboard shortcuts list | Part | Yes | Yes |
| 3D mouse | No | Yes | No |

## 3. Where PrintCraft is ahead

Keep these when closing gaps:

* **Connectivity.** One app covers the Creality network service, Moonraker, PrusaLink,
  USB (Marlin, Repetier-Firmware) and Sailfish. It has discovery, re-finding a printer
  that changed address, and cameras. PrusaSlicer has no USB printing and no cameras.
* **Old machines.** X3G export and live Sailfish control. PrusaSlicer has neither.
* **Projects.** A project remembers the printer device and reconnects to it.
* **Simplicity.** One screen, three quality presets, few settings. Section 4's gaps
  should land behind an "Advanced" switch, not add to the default view.

## 4. Gap ranking

The score is impact on print quality or daily use (H/M/L), cost (S = under 1 day,
M = 1-3 days, L = 1-2 weeks, XL = over 2 weeks), and whether it keeps the app simple.

| # | Gap | Impact | Cost | Notes |
|---|---|---|---|---|
| 1 | Bridge detection and bridge settings | H | M | Biggest visible quality difference |
| 2 | Per-feature speeds and accelerations | H | M | Outer wall slower than infill is standard |
| 3 | Custom start/end G-code with placeholders | H | M | Needed for most non-preset printers |
| 4 | Undo / redo | H | M | Basic editor expectation |
| 5 | Gap fill and avoid crossing walls | H | M | Stringing and thin-wall holes |
| 6 | Colour change / pause at layer | M | S | Small, common request |
| 7 | Flow ratio, overlap, elephant foot, hole compensation | M | S | Calibration knobs |
| 8 | Preview colour by speed/flow/fan/temp, per-feature time | M | M | Needed to check items 1-2 |
| 9 | More infill (gyroid, honeycomb, cubic, lightning) | M | M | |
| 10 | Per-object settings and height ranges | H | L | Needs a settings-overlay model |
| 11 | Support enforcers/blockers, manual supports | M | L | Needs painting or picking |
| 12 | Mesh repair and non-uniform scale | M | M | |
| 13 | Cut tool, split, pick-a-face lay flat | M | L | |
| 14 | Variable layer height | M | L | Needs per-layer heights in the slicer |
| 15 | Tree supports | M | XL | |
| 16 | Arachne variable-width walls | M | XL | Large algorithm |
| 17 | Object labels, M73, arc fitting, binary G-code | M | M | Output features |
| 18 | OctoPrint, Duet hosts | M | M | Fits PrintCraft's strength |
| 19 | Filament library, user presets, import/export | M | M | |
| 20 | Sequential printing, multiple plates | L | L | |
| 21 | Windows/macOS builds | M | M | Unverified today |
| 22 | Multi-material (MMU), IDEX, tool changers | L | XL | Hardware-specific |
| 23 | STEP, text emboss, measure, SLA, translations | L | XL | Out of scope for a simple app |

Items 22-23 are listed for completeness. `PCGAPPLAN.md` leaves them as "not planned"
unless asked.
