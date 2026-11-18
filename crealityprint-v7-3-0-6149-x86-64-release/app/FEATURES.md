# PrintCraft compared with PrusaSlicer's feature set

The person asked to match PrusaSlicer's features, as listed on prusa3d.com/p/prusaslicer.
That page answered the automated browser with a bot check ("Just a moment…", HTTP 403),
so it couldn't be read. Getting around such checks isn't appropriate. The list below is
PrusaSlicer's widely documented feature set from general public knowledge, not from that
page. If the page lists something not here, save it (or a screenshot) into `reference/`.

Status:
* **Yes**: PrintCraft does it.
* **Simpler**: a smaller version of the same idea.
* **Not yet**: left out this round.

## Preparing models

| Feature | PrintCraft |
|---|---|
| STL import | Yes |
| OBJ import | **Yes (new)** |
| 3MF import (meshes, components, build transforms) | **Yes**. This includes Bambu Studio / Orca / PrusaSlicer project files that keep each object in its own part (`3D/Objects/object_1.model`, linked with the 3MF Production extension's `p:path`). Items marked not printable are skipped. Colours, settings and paint data inside 3MF are ignored. |
| STEP, AMF import | Not yet |
| Selection marks and handles in the 3D view | **Yes (new)**. A selected model gets white corner brackets and its size above it. A tool strip on the left (Move M, Scale S, Rotate R) chooses the handles: red and green arrows move along X or Y; orange corner squares scale evenly (there is no separate X/Y/Z stretch); red, green and blue rings turn the model about X, Y or Z. There is no up/down arrow because models always rest on the plate. |
| Move, rotate, scale, arrange, duplicate | Yes |
| Lay on face | **Simpler (new)**: "Lay flat" puts the biggest flat face down; you can't pick a face |
| Cut tool, modifiers, height range modifiers, per-object settings | Not yet |
| Variable layer height | Not yet |
| Multiple plates, sequential (one-at-a-time) printing | Not yet |

## Projects (src/project.rs, src/app/project_ui.rs)

The **File** menu at the top left has New project, Open project…, Save project (Ctrl+S)
and Save project as… (Ctrl+Shift+S), plus Add model and Export G-code. A project file
(`.printcraft`, JSON) holds:
* every object on the plate: name, shape (stored in the file, so the original STL/3MF
  may move), position, rotation and scale
* all process settings, the quality preset and the filament with its temperatures
* the printer profile with all its edited values (plate size, nozzle, retraction, purge,
  Klipper)
* the printer device in use (address or USB port, name, connection kind; never a
  password)

When a project opens, the plate and settings are restored straight away. The saved
printer is then looked for in the background: its USB port, or its network port. If it
answers it is connected and selected. If not, **no printer is selected and a "Printer not
found" alert** says which printer couldn't be located. A project without a printer leaves
none selected. New and Open ask "Save changes?" first when the plate has unsaved changes.
Projects can also be opened by dropping them on the window or naming them on the
command line, and the window title shows the project's name.
**Recent projects** (person's request, `src/recent.rs`): the File menu lists the 5
projects opened or saved most recently, newest first and numbered, so one click opens
them. Hovering shows the full path. A project that was moved or deleted is greyed out,
and is dropped from the list if opening it fails. "Clear recent projects" empties the
list. The list is kept in `recent_projects.json` in the settings folder, beside the saved
printers. Missing or unknown values in
a project file fall back to defaults, so older and newer files still open.

## Slicing

| Feature | PrintCraft |
|---|---|
| Layer height, walls, top/bottom layers, infill density, speeds | Yes |
| Speeds per feature (outer/inner wall, sparse/solid infill, top surface, support) | **Yes (new)**, in Advanced; each "Auto" until set |
| Accelerations (outer wall, infill, travel, first layer) | **Yes (new)**, in Advanced; M204 or Klipper SET_VELOCITY_LIMIT, only when set. The time estimate models acceleration and corners |
| Quality presets | Yes (Fine / Standard / Draft). They now change only quality values, so your other choices stay |
| Infill patterns | **Simpler (new)**: Lines, Grid, Triangles. Not offered: gyroid, honeycomb, cubic, lightning, adaptive cubic, support cubic |
| Arachne variable-width walls | Not yet (fixed-width walls) |
| Seam position (nearest / aligned-rear / random) | **Yes (new)** |
| Seam and support painting | Not yet |
| Ironing of top surfaces | **Yes (new)**: 15 % flow, 15 mm/s, 0.2 mm spacing, topmost surface layers only |
| Fuzzy skin | **Yes (new)**: 0.3 mm wobble every 0.8 mm, outer walls, first layer excluded |
| Spiral vase mode | **Yes (new)**: one wall, solid bottom, continuous rising Z |
| Supports (grid) with overhang angle | Yes |
| Supports "on build plate only" | **Yes (new)** |
| Organic (tree) supports, support enforcers/blockers | Not yet |
| Skirt and brim | Yes |
| Raft | **Yes (new)**. Adhesion "Raft" puts 3 raft layers under the whole print, reaching 3 mm past it, with a 0.15 mm gap to the model. With supports on, support columns that reach the bed stand on their own small raft, even without the Raft option. A "Raft under support" switch in the Support section (on by default) turns this off. |
| Retraction | Yes: before travels over a set length (2 mm by default). Optional: on every layer change, and a wipe back along the last line while retracting. These are set in the printer editor. |
| Flow ratio, infill/wall overlap, elephant-foot and hole compensation | **Yes (new)**: flow ratio per filament; the others in Advanced. All default to today's values |
| Avoid crossing walls on travel | **Yes (new)**, in Advanced (off by default): travels go round inside the part instead of over air, and those aren't retracted |
| Z-hop (lift on retraction) | **Yes (new)** |
| Cooling: slow down short layers (minimum layer time) | **Yes (new)**: default 15 s, never below 15 mm/s |
| Fan control | Yes (filament fan %, off on the first layer) |
| Gap fill (thin walls and spaces too narrow for another wall) | **Simpler (new)**: one line down the middle of the space, as wide as the space |
| Bridge detection and settings | **Simpler (new)**: unsupported areas anchored at both ends are filled with solid lines across the shortest anchored span (own colour in the preview). Bridge speed (Auto: half the print speed), flow (95 %) and fan (100 %) are their own settings in Advanced |
| Multi-material / multi-tool (MMU, XL tool changer, wipe tower) | Not yet |
| SLA (resin) printing | Not yet |

## Output and preview

| Feature | PrintCraft |
|---|---|
| 3D layer preview with a slider, colour by feature type | Yes (ironing is now its own colour) |
| Time, filament, weight and cost estimates | Yes |
| Plain G-code export | Yes |
| Binary G-code (.bgcode) | Not yet (PrusaLink accepts plain G-code) |
| Printer profiles | **More (new)**: Prusa MK4S, Core One, MK3S+, MINI+, XL (one tool) and Creality Ender-3 V3 KE, plus four generic sizes |

## Printers and connections

| Feature | PrintCraft |
|---|---|
| Send to printer over the network | Yes: Creality network service and Moonraker/Klipper, plus **PrusaLink (new)** |
| Find printers on the network | Yes (address scan; now also finds PrusaLink printers) |
| Known printer with a new address | **Yes (new)**. Connecting to a known printer, or opening a project with one, checks the saved address first. If the printer isn't there, or a different printer is, the local network is searched for the printer by its kind and name. The app connects at the new address and updates the known list. |
| Monitor progress, layers, time and temperatures; pause/resume/stop | Yes (Creality, Moonraker, PrusaLink, USB) |
| Camera view | Yes for Creality/Klipper MJPEG cameras; Prusa cameras use the Prusa Connect cloud (not supported) |
| USB printing (stream G-code over a cable) | **Yes (new)**: Marlin/Prusa firmware, line numbers, checksums, resend. Every line sent and received is recorded in `usb-serial.log` in the settings folder's `logs` folder, for diagnosing a printer. |
| MakerBot protocol / Sailfish printers (FlashForge Creator Pro, early Creator Pro 2, Replicator) | **Yes (new)**, over USB: choose "MakerBot / Sailfish (X3G)" under USB. Prints are translated to X3G. They are recorded onto the SD card where the firmware allows; stock Sailfish doesn't, so they are streamed over USB. Pause, resume, stop (heaters off), temperatures, part fan, jog and home are supported, as are the SD card's file list and printing from it. The page shows the firmware, both nozzles' temperatures and the machine's settings (read from its EEPROM, saved as `sailfish-eeprom.bin`). X3G export saves files for the SD card. Not yet confirmed on a real printer. |
| Two nozzles | **Yes (new)**, for two nozzles on one carriage. Set "Nozzles" to 2 in the printer editor (the Creator Pro preset has 2). A "Primary nozzle" setting (right, T0, by default) picks the default nozzle. Each part gets a nozzle, supports get their own, and the other nozzle gets its own filament. There is one tool change per layer: the idle nozzle is pulled back and kept at a standby temperature, then reheated and primed. Ditto printing is available. One-nozzle printers are unchanged. A prime tower (on by default) and an optional ooze shield around the print take the ooze from tool changes and the idle nozzle. Filament and cost are shown per nozzle, and the preview can colour by nozzle and shows the Ditto copy. Not yet confirmed on a real printer. |
| Repetier-Firmware printers | **Yes (new)**, over USB. Found by its M115 name. The printer's screen gets print mode, the job name and progress (M530/M531/M532). Temperatures are polled while printing. Calibration uses G32. Its replies (`rs:`, `wait`, `fatal:`) are understood. The printer page shows a "Repetier-Firmware v1" or "v2" badge (from the M115 version). Repetier-Server (network) is not supported. |
| Prusa Connect (cloud), OctoPrint, Duet, other hosts | Not yet |

All new slicing values are listed in MECHANICS.md and mechanics.json. The connection
details are in PRINTER_LINK.md.
