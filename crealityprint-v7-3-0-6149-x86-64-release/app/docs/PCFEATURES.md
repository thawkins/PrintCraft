# PrintCraft feature set

This is the complete list of what PrintCraft does as of version 0.0.12. It is a
catalogue of features, not a comparison with other slicers: for that, see
[FEATURES.md](../FEATURES.md) and [PCGAP.md](PCGAP.md). The numbers behind slicing are
in [MECHANICS.md](../MECHANICS.md), and how each printer connection works is in
[PRINTER_LINK.md](../PRINTER_LINK.md).

Features marked *experimental* work but are off by default until they have been tested
on prints. Features marked *unconfirmed* are built from published protocol documents
but have not yet been tried on a real printer.

## 1. Application

- Desktop app written in Rust with egui/eframe, with a dark theme and a teal accent.
- Two main screens: **Prepare** (plate, settings, slicing and preview) and **Printer**
  (connections, status and control).
- An **Advanced** switch shows the full set of print settings; with it off, only the
  common ones are shown. Every setting has a hint, shown when hovering over its label or
  field.
- Models and projects can be dropped onto the window or named on the command line.
- File dialogs don't stop the window from responding.
- Runs on Linux, Windows and macOS; every change is built and tested on all three, and
  each slices the same calibration cube to the same G-code byte for byte.
- On narrow windows the model toolbar wraps onto a second row and the Move / Scale /
  Rotate strip moves under the view buttons, so nothing overlaps.

## 2. Models and the plate

### Import

- STL (binary and text), OBJ and 3MF.
- 3MF meshes, components and build transforms, including project files from other
  slicers that keep each object in its own part (3MF Production extension `p:path`).
  Items marked not printable are skipped.
- 3MF units: inches, centimetres, metres, feet and microns load at the right size.
- Damaged input is handled safely: models with NaN or infinite coordinates are
  rejected, and a bad OBJ face or text STL facet drops only itself.
- Built-in shapes from **Add shape**: cube, cylinder, cone and tube.

### Mesh repair on import

- Merges points less than 0.0001 mm apart.
- Removes faces collapsed to a line or a point.
- Turns faces the right way out, so neighbours agree and closed parts face outwards.
- Closes simple holes.
- The status bar says what was fixed. A clean model is left exactly as it was.

### Placing and editing

- Selection marks: white corner brackets and the part's size above it.
- Tool strip with on-screen handles:
  - **Move (M)**: X and Y arrows, or drag the middle to move freely. Parts always rest
    on the plate.
  - **Scale (S)**: corner handles scale evenly.
  - **Rotate (R)**: rings turn the part about the world X, Y and Z axes.
  - **Face (F)**: highlights the flat face under the mouse and lays the part on it with
    a click.
- Objects tab: typed position, rotation and scale. With **Lock** unticked, X, Y and Z
  scale separately along the model's own axes.
- Lay flat (biggest flat face down), Turn 90°, Center, Duplicate, Delete, Arrange and
  Clear plate.
- **Cut**: a horizontal cut at a chosen height, keeping both parts, the lower or the
  upper. Cut faces are closed (holes included), and the upper part is turned onto its
  cut face. An orange plane shows where the cut will go.
- **Split**: makes each separate piece of a part its own object, where it stood. The
  inside wall of a hollow stays with the piece around it.
- Undo and redo (Edit menu, Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z): 100 steps of plate,
  settings, printer, filament, two-nozzle and colour-change changes. A whole drag, or a
  typed value, is one step.
- 3D view with Home, Front and Top views.

### Per-object settings

- **Settings for this object** on the Objects tab. A part can override the plate's
  walls, top and bottom layers, infill density, pattern and overlap, ironing, fuzzy
  skin, and elephant-foot and hole compensation. Each override has a button that resets
  it to the plate's value. The object list shows a badge such as "2 settings".
- **Height ranges**: different per-object settings between two heights of a part. Where
  ranges overlap, the lower one in the list wins.
- **Modifier volumes**: a box, cylinder or sphere on a part. Inside it, the infill
  density and pattern can differ (100 % prints as solid infill). Modifiers are drawn as
  see-through blue volumes.
- **Support blockers** (red) stop supports inside them. **Support enforcers** (green)
  support every downward-facing surface inside them, even with supports off.
- Overrides, ranges and modifiers are saved in projects and kept by split parts and by
  the lower part of a cut.

## 3. Projects

- **Several plates** per project: tabs under the model toolbar switch plates, "+" adds
  one and "×" removes an empty one. Each plate is sliced, previewed, exported and sent
  on its own and keeps its slice while another is shown. Undo and redo cover the plate
  being worked on.
- `.printcraft` project files (JSON) hold:
  - every plate's objects, with each mesh stored in the file and its position, rotation,
    scale, overrides, height ranges and modifiers (a one-plate file is the same as
    before plates; older versions open plate 1 of a larger one, with a warning);
  - all print settings, the quality preset, and the filament(s);
  - the printer profile with every edited value, including custom G-code;
  - the printer device in use (address or port, name and kind). Passwords are never
    saved;
  - colour changes and pauses.
- New, Open, Save (Ctrl+S) and Save as (Ctrl+Shift+S). New and Open ask "Save changes?"
  when the plate has unsaved changes.
- Opening a project reconnects to its printer in the background. If the printer can't be
  found, a "Printer not found" alert names it.
- **Recent projects**: the File menu lists the 5 most recent, numbered. Hovering shows
  the path, and moved or deleted ones are greyed out. **Clear recent projects** empties
  the list.
- Older and newer project files open: missing values use defaults, and out-of-range
  values are clamped. A project from a newer version shows a warning.
- Saves are atomic: the file is flushed before it replaces the old one, and a failed
  save leaves no temporary file.

## 4. Print settings

### Quality, layers and filament

- Fine, Standard and Draft quality presets, which change only quality values.
- Layer height, first layer height and **variable layer height** (Advanced): thinner layers on
  gentle slopes and thicker layers on steep walls, from half to 1.5 times the layer
  height. Quality and smoothing are settable. One profile is used for the whole plate.
- Filament presets: PLA, PETG, ABS, TPU and ASA. Each has nozzle and plate
  temperatures, diameter, density, fan, price per kg and a flow ratio.
- **Your own presets**: "Save as preset…" keeps the current filament or process settings
  under a name; they appear in the filament and process lists, can be deleted, and are
  exported and imported as a JSON file. Kept in `presets.json` in the settings folder.

### Walls and surfaces

- Wall count and top and bottom layer counts.
- Seam position: aligned, nearest, rear or random.
- **Gap fill**: one line, as wide as the space, down spaces too narrow for another wall
  or for infill.
- **Variable-width walls** (*experimental*): walls follow the shape where they fit. Thin
  parts get a centre line as wide as the space, and walls widen over gaps too thin for a
  line.
- **Top surface pattern** and **Solid infill pattern**: Lines, Monotonic or Concentric.
- **Ironing**: a light pass over the topmost surfaces (15 % flow, 15 mm/s, 0.2 mm
  spacing).
- **Fuzzy skin**: 0.3 mm wobble every 0.8 mm on outer walls, except on the first layer.
- **Textured top surface** ("fuzzy top"): a random lift of up to 0.1 mm.
- **Spiral vase mode**: one wall, a solid bottom and a continuously rising Z.
- Calibration values: infill/wall overlap, elephant-foot compensation (the first layer
  is drawn in, except over a raft) and hole compensation (holes are made larger).

### Infill

- Density, from hollow to solid.
- Patterns: Lines, Grid, Triangles, Gyroid, Honeycomb, Cubic, Adaptive cubic (half
  density more than 4 mm inside a part) and Lightning (branches only under top
  surfaces).

### Bridges

- Unsupported areas anchored on both sides are filled with solid bridge lines across
  the shortest span, reaching 2 mm onto the supported area.
- Bridge speed, bridge flow (95 %) and bridge fan (100 %) are their own settings.

### Supports and adhesion

- Supports under overhangs past a settable angle, optionally only from the plate.
- Support styles: **Grid** columns or **Tree** branches. Tree branches lean (at most
  40°) towards each other and join, keep clear of the model, and have settable tip and
  branch diameters.
- Top and bottom Z gap, XY gap, line spacing and interface layers.
- Support columns that reach the plate stand on a small raft. **Raft under support** can
  turn this off.
- Adhesion: none, skirt, brim or **raft**. A raft is 3 layers that reach 3 mm past the
  print, with a 0.15 mm gap to the model.

### Speed and motion

- Print, first-layer and travel speeds, plus per-feature speeds (Advanced): outer wall, inner wall, sparse
  infill, solid infill, top surface, support and bridge. Each is "Auto" until set.
- Accelerations (Advanced) for outer wall, infill, travel and first layer. They are
  written as `M204` or Klipper `SET_VELOCITY_LIMIT`, and only when set.
- No printing move is slower than 15 mm/s.

### Retraction, travel and cooling

- Retraction on travels over a settable length (2 mm by default), optionally on every
  layer change, and an optional wipe back along the last line.
- Z-hop on retraction.
- **Avoid crossing walls** (Advanced): travels go round inside the part rather than out
  over air, so they need no retraction.
- Fan control (filament fan %, off on the first layer).
- Minimum layer time: short layers slow down (15 s by default, never below 15 mm/s).

## 5. Two-nozzle printing

For printers with two nozzles on one carriage (FlashForge Creator Pro, Replicator Dual,
and similar).

- A nozzle for each part, a support nozzle, and a second filament.
- A settable primary nozzle (right, T0, by default) and nozzle spacing.
- One tool change per layer. The idle nozzle is pulled back (tool-change retraction) and
  kept at a standby temperature, then reheated and primed.
- A **prime tower** (on by default, with a settable position) and an optional **ooze
  shield** around the print.
- **Ditto printing**, with a check that the job is narrow enough for the copy and that
  the copy stays on the plate.
- Filament, weight and cost per nozzle. The preview can be coloured by nozzle and shows
  the Ditto copy.
- *Unconfirmed* on a real printer.

## 6. Printer profiles

- Presets: Prusa MK4S, Core One, MK3S+, MINI+, XL (one tool), Creality Ender-3 V3 KE,
  FlashForge Creator Pro (Sailfish, two nozzles), and four generic plate sizes.
- The printer editor covers plate size, nozzle diameter, number of nozzles, nozzle
  spacing, retraction and wipe, purge amount, acceleration, Klipper firmware, MakerBot /
  Sailfish (X3G) output, binary G-code (`.bgcode`), and the print head's clearance
  radius and gantry height for sequential printing.
- **Custom G-code** per printer: start, end, before each layer change, and at each tool
  change. Each has an **Insert default** button.
  - 26 placeholders such as `{bed_temp}`, `{nozzle_temp}`, `{layer_num}`, `{layer_z}`,
    the print's bounds, and `{tool}`/`{next_tool}`.
  - `{name-1}`-style offsets and `{if}`/`{else}`/`{endif}`.
  - Mistakes stop the slice with a message naming the box and the tag.

## 7. Slicing output

- Plain G-code export, **binary G-code** (`.bgcode`: MeatPack and heatshrink, CRC32
  blocks, thumbnails and job details) when the printer profile asks for it, and **X3G**
  export for MakerBot / Sailfish printers (the export button says "Export X3G").
- **Sequential printing** (optional): one object at a time, shortest first, lifting clear
  and moving across between objects. Slicing is refused, with a message naming the
  objects, when they are too close for the print head or more than one is taller than
  the gantry.
- **Object labels** (optional): Klipper `EXCLUDE_OBJECT` or M486, so one object can be
  cancelled while the others go on.
- **Arc fitting** (optional): round walls written as G2/G3 arcs, within 0.01 mm of the
  sliced points.
- PNG thumbnails embedded in the G-code for printer screens and Klipper web pages.
- `M73` progress (percent and minutes left) every layer for Marlin and Prusa firmware.
- Klipper printers are told the layer count (`SET_PRINT_STATS_INFO`), so their screens
  show "layer 9 of 150".
- Heating waits only while heating up. A hotter nozzle or plate is not left to cool.
- Optional bed levelling before a print (`BED_MESH_CALIBRATE` on Klipper, `G29` on
  Marlin and Prusa, `G32` on Repetier), after both the nozzle and the plate have heated.
- **Colour changes and pauses** at chosen layers, added from the preview: `M600` for a
  colour change, and `PAUSE` (Klipper) or `M601` for a pause. They are marked on the
  layer slider, saved in the project, and need no new slice. They are not offered on
  MakerBot / Sailfish printers.
- Slicing runs in the background. The window and the command line share one slicing
  pipeline.

## 8. Preview and estimates

- 3D layer-by-layer preview with a layer slider.
- **Colour by**: line type, nozzle, speed, line width, layer height, fan or temperature,
  with a blue-to-red scale bar for the value modes.
- Print time per line type, plus Travel and "Heating, purge, changes" rows, adding up to
  the total.
- Estimates for print time (modelling acceleration and corners), filament length,
  weight and cost.

## 9. Printers and connections

### Network printers

- Creality network service, Klipper (Moonraker), PrusaLink (HTTP Digest sign-in),
  OctoPrint (API key) and Duet / RepRapFirmware (standalone `rr_` requests, or the
  `/machine` API when run by a Raspberry Pi). The PrusaLink password, OctoPrint key or
  Duet password goes in one "Password or API key" box and is never saved. PrusaLink,
  OctoPrint and Duet are *unconfirmed*: built from their published APIs and tested
  against pretend servers, not yet on real printers.
- PrusaLink printers can be sent binary G-code (`.bgcode`).
- **Find printers**: scans the local /24 network and identifies each printer found.
- Known printers are remembered, and can be forgotten. A known printer that has moved to
  a new address is found again by its kind and name.
- Uploads a job and starts it.

### USB printers

- Streams G-code to Marlin, Prusa and Repetier firmware, with line numbers, checksums
  and resend.
- Asks the firmware for its name (`M115`) and shows a protocol badge: "Marlin-style
  protocol", "Repetier-Firmware v1" or "v2".
- Repetier extras: print mode, job name and progress on the printer's screen
  (`M530`/`M531`/`M532`).
- Understands `ok N`, `Resend`, `rs:`, `wait`, `busy:`, `start` and `fatal:` replies.
- Every line sent and received, with timings, is logged to `usb-serial.log` in the
  settings folder's `logs` folder. The previous log is kept as
  `usb-serial.previous.log`.
- **MakerBot / Sailfish** printers (FlashForge Creator Pro, early Creator Pro 2,
  Replicator and clones), *unconfirmed*:
  - prints are translated to X3G and recorded to the SD card where the firmware allows,
    otherwise streamed;
  - firmware version, both nozzles' temperatures and the machine settings, read from
    its EEPROM and saved as `sailfish-eeprom.bin`;
  - pause, resume, stop (heaters off), temperatures, part fan, jog and home;
  - the SD card's file list, and printing from it.

### The Printer page

- Printer tab with the printer's name and address.
- Live status: job state, progress, layers, time left, end time and temperatures.
- Print settings while printing: temperatures, model fan and printing speed.
- Pause, Resume and Stop (Stop asks first), and **Print again** when a print finishes.
- **Cancel object** on Klipper and Duet: the objects of a labelled print are listed,
  each with a button that asks first.
- Tabs:
  - **XYZ Control**: jog and home;
  - **Local Files**: files on the printer, which can be printed;
  - **Records**: past prints;
  - **Timelapse Video**: a placeholder, because how printers share timelapse videos isn't
    publicly documented.
- **Camera** view for Creality and Klipper MJPEG cameras, which can be enlarged or
  maximised.
- Connection status reported the same way for every printer: "Connected.", "Lost
  contact: …" or "Not connected: …".

## 10. Command line

- `printcraft [MODEL ...]`: open the app with models.
- `--preview`: open the models and slice them straight away.
- `--slice MODEL ... --output OUT.gcode|OUT.bgcode|OUT.x3g`: slice without a window.
  Options:
  - `--quality fine|standard|draft`
  - `--filament pla|petg|abs|tpu|asa`
  - `--printer 1-11`
  - `--supports`, `--tree-supports` and `--no-support-raft`
  - `--brim` and `--raft`
  - `--infill-pattern lines|grid|triangles|gyroid|honeycomb|cubic|adaptive-cubic|lightning`
  - `--seam aligned|nearest|rear|random`
  - `--top-pattern` and `--solid-pattern`, each `lines|monotonic|concentric`
  - `--ironing`, `--vase`, `--fuzzy`, `--fuzzy-top` and `--variable-walls`
  - `--label-objects`, `--arc-fitting` and `--sequential`
- `--connect ADDRESS|PORT`: open the Printer page and connect over the network or USB
  (115200 baud). Add `--camera big|max` for an enlarged camera.
- `--device`, `--tool move|scale|rotate` and `--file-menu`: open on a given page, tool
  or menu.
- `--screenshot OUT.png`: draw the window, save a picture and quit.
- `--help` and `--version`.

## 11. Not supported yet

- STEP and AMF import. 3MF colours, settings and paint data are ignored.
- Seam and support painting.
- Walls and other settings inside modifiers, imported meshes as modifiers, and
  modifiers that turn with their part.
- Variable layer height per object.
- Multi-material units, tool changers other than two nozzles on one carriage, and wipe
  towers for them.
- SLA (resin) printing.
- Prusa Connect (cloud), Repetier-Server and other print hosts.
- Prusa cameras, which use the Prusa Connect cloud.
- Listing or downloading timelapse videos.
