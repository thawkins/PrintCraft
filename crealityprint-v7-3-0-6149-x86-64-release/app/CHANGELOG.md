# Changelog

All notable changes to PrintCraft are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Printing to MakerBot / Sailfish printers over USB. The plate is translated to X3G and
  recorded onto the SD card where the firmware allows, otherwise streamed. Stock
  Sailfish can't record, so it streams. Also:
  * pause, resume and stop (the queue is emptied and the heaters switched off);
  * nozzle and bed temperatures, the part fan, jog and home;
  * the SD card's file list, and printing a file from it.

  Not yet confirmed on a real printer. (fe2826b)
- Two-nozzle printing, for two nozzles on one carriage (Creator Pro, Replicator Dual).
  A printer's "Nozzles" setting (the Creator Pro preset has 2) shows:
  * a nozzle choice for each part;
  * the support nozzle;
  * a second filament;
  * Ditto printing.

  Each layer prints one nozzle's paths, then the other's, with a retract-switch-prime
  tool change. The second nozzle is heated, purged and switched off with the first. X3G
  files carry the tool changes. One-nozzle printers get the same G-code as before. (fe2826b)

- Retraction settings in the printer profile:
  * "Retract on travels over" (the travel length that retracts; 2 mm as before);
  * "Retract on layer change";
  * "Wipe" (retract while moving back along the line just printed).

  The defaults keep the G-code exactly as before. (fe2826b)
- Anti-ooze for two nozzles. The nozzle being set aside is pulled back by a
  "Tool-change retraction" (8 mm), dropped to a standby temperature ("Standby drop",
  15 °C below printing), then heated back and primed by the same amount before it prints
  again. The second nozzle is also set aside after its start-up purge. (fe2826b)
- A prime tower for two-nozzle prints ("Prime tower" under Two nozzles, on by
  default): a 12 mm square near the back-right corner. The incoming nozzle primes on it
  after each tool change, up to the last layer with a change. Its position can be set
  ("Tower position", mm from the front-left corner). It must stand on the plate and clear
  of parts. (fe2826b)
- An ooze shield for two-nozzle prints ("Ooze shield" under Two nozzles, off by
  default). It is a one-line wall a set distance (3 mm) outside everything the print lays
  down, skirt, brim and raft included, for the idle nozzle to wipe on.
  * It is the same on every layer, so it never overhangs, and is built up to the last
    layer with a tool change.
  * Each layer prints it first, with the nozzle already selected, so it adds no tool
    changes.
  * A shield that would run off the plate, or a prime tower inside it, is refused after
    slicing. (fe2826b)

- Two-nozzle prints show filament, weight and cost for each nozzle, worked out with
  each nozzle's own filament. The preview can colour paths by nozzle, and shows the Ditto
  copy dimmed beside the part. (fe2826b)
- The printer editor scrolls within the space above the Move / Scale / Rotate buttons,
  instead of running under them now that it has more rows. (fe2826b)
- On X3G printers the export button and File menu item say "Export X3G". On two-nozzle
  printers the filament chips say which nozzle each filament is in. (fe2826b)
- Ditto checks:
  * The job as sliced, skirt, brim and raft included, must be narrower than the nozzle
    spacing less 3 mm, so the two copies can't print into each other. A job that isn't
    shows why, and can't be sent or exported.
  * A copy that would print off the plate is refused.
  * The spacing is a printer setting ("Nozzle spacing", 33 mm by default for the Creator
    Pro family). (fe2826b)

- AGENTS.md and AGENTS-LANGUAGE.md in the app folder, copied from the ragent project.
  (159e565)

### Changed

- Two-nozzle printers have a primary nozzle: a "Primary nozzle" setting in the printer
  editor, right (T0) by default, the firmware's tool 0. New parts, supports, the skirt and one-nozzle prints use
  it, and it has the main filament ("01"). The other nozzle is heated only when something
  uses it. Changing the primary moves the parts and supports that were on the old one. (fe2826b)
- The G-code header now says the plate size (`; plate: 227.0 x 148.0 mm`), which the X3G
  translator reads. (fe2826b)

## [0.0.4] - 2026-11-18

### Fixed
- X3G files, after a command-by-command comparison with GPX 2.6.8:
  * The part fan (`M106`/`M107`) now switches the board's "extra" output. Before, it
    switched the nozzle's heatsink fan, and turned it off at the start of every print.
  * Heat waits no longer give up after 10 minutes and print cold.
  * Homing X and Y runs at the same speed as GPX.
  * A move far shorter than one step can no longer ask for a step rate above the axis's
    limit. (0f27131)

### Added
- X3G export for MakerBot / Sailfish printers. The new "FlashForge Creator Pro
  (Sailfish)" preset, or the "MakerBot / Sailfish" printer setting, makes Export save an
  `.x3g` file for the printer's SD card. The command line does the same with
  `--output OUT.x3g --printer 11`.
  * The G-code is translated to MakerBot S3G commands: moves in motor steps from the
    bed's centre, heating and waits, homing, and progress on the printer's screen.
  * Steps per mm start from the Replicator 1 Dual family's values. Once the printer has
    been connected over USB, its own saved settings are used instead.
  * Single nozzle for now. Not yet printed on a real printer. (6c6f163)
- MakerBot-protocol (Sailfish) printers over USB, read-only. Under USB, choose the
  protocol "MakerBot / Sailfish" to connect to a FlashForge Creator Pro, early Creator
  Pro 2, Replicator or a clone. The app shows:
  * the firmware, with a "MakerBot protocol · Sailfish x.y" badge;
  * temperatures for both nozzles and the bed;
  * a print running from the printer's SD card.

  It reads the machine's settings from its EEPROM (steps per mm, axis lengths, nozzle
  count, Ditto printing) and saves a copy as `sailfish-eeprom.bin`. Every packet goes
  into `usb-serial.log` in hex, with its meaning. Nothing that moves, heats or writes is
  sent yet. Not yet confirmed on a real printer. (91f4d92)
- makerbot.md: the plan for X3G export, MakerBot/Sailfish printing, FlashForge printers
  and configurable dual extruders, with the protocol facts checked against the published
  sources. Dual-extruder printing itself is planned there, not yet built. (91f4d92)
- BUILDSTATS.md: build times, disk use, line coverage per module and test totals from a
  measured run. (2081fb6)
- The ORBIT print-in-place planetary gear model (by albioli on Thingiverse, CC BY-NC-ND)
  as a heavy test model in `assets/models`. (a9fc78f)

## [0.0.3] - 2026-11-18

### Added
- Repetier-Firmware printers over USB, recognised from their M115 name. The printer's
  screen shows print mode, the job name and progress (`M530`/`M531`/`M532`),
  temperatures are asked for while printing, and calibration uses `G32`. Not yet
  confirmed on a real printer. (b6c250b)
- A USB printer's page shows which protocol drives it: a "Repetier-Firmware v1" or
  "Repetier-Firmware v2" badge, or "Marlin-style protocol", and the activity log says so
  when the printer names its firmware. (b6c250b)
- USB connections record every line sent and received, with timings, in `usb-serial.log`
  in the settings folder's `logs` folder (the log before is kept as
  `usb-serial.previous.log`), so a misbehaving printer can be diagnosed. (b6c250b)
- A known network printer that has moved to a new address is found again: the saved
  address is tried first, then the local network is searched for a printer of the same
  kind and name, and the known list is updated. (b6c250b)
- `--connect` also takes a serial port (`/dev/ttyUSB0`, `COM3`) and connects over USB.
  (b6c250b)

### Changed
- Every USB printer is asked for its firmware (`M115`) after connecting, and more
  replies are understood: `ok 12`, `Resend: N12`, `rs:12`, `wait` (a lost `ok`),
  `busy:`, `start` (the board restarted, which ends a print) and `fatal:` (ends a
  print). (b6c250b)
- The repository tracks the reference pictures, investigation notes and project JSON
  files, and ignores lock files (except `Cargo.lock`) and agent session transcripts.
  (59038ce, 6536e3e, 03f769b, aeb17af)

## [0.0.2] - 2026-11-17

### Added
- Recent projects: the File menu lists the 5 projects opened or saved most recently,
  greys out ones that were moved or deleted, and has "Clear recent projects".
  (b80d8f9)
- Raft adhesion: 3 raft layers under the whole print, reaching 3 mm past it. Support
  columns that reach the bed stand on their own small raft ("Raft under support" in
  the Support section, on by default). (b80d8f9)
- Drawn icons for the Move, Scale and Rotate tools. (b80d8f9)
- More tests (146 in all), covering geometry, presets, saved printers, the slicing
  pipeline, Klipper status, USB parsing, camera frames, the 3D view, base64, recent
  projects and rafts. (5cb7ab4, b80d8f9)
- Tests run with cargo-nextest (`cargo nextest run`), configured in
  `.config/nextest.toml`, with a `ci` profile that retries once and writes JUnit.
  (5cb7ab4)
- Opening a project saved by a newer PrintCraft warns that it may use features this
  version doesn't support, and that saving it here drops them.
- 3MF files in inches, centimetres, metres, feet or microns now load at the right size
  (the `unit` of the model was ignored before).
- Setting ranges are defined once and shared by the settings panel and the project
  loader.
- A project's "Save As" adds `.printcraft` to the name instead of replacing what follows
  a dot.

### Changed
- Version numbers start at 0.0.1; this is the first numbered release after it.
  (2421197, 73a26c8)
- The repository's root README shows how to build and run PrintCraft. (3348e50)
- Every printer connection (Creality, Klipper, PrusaLink, USB) reports contact the same
  way: "Connected.", "Lost contact: …" or "Not connected: …", each problem once instead
  of on every retry.
- Dialogs have 10 px more room between their edges and the text.
- No printing move is slower than 15 mm/s, including speeds set in the settings; the
  speed fields now start at 15 mm/s.
- The window and the command-line slicer share one slicing pipeline, so the plate-fit
  check and line width can no longer differ between them.
- The X and Y rotate rings now turn a model about the world axes they are drawn around,
  also after the model has been turned about Z.
- Pressing the middle of the move handles moves the model freely instead of along X.
- Lay flat treats only faces in the same plane as one face, and settles ties the same
  way every time.
- Bed levelling before a print is placed after both the nozzle and the plate have
  heated, whichever comes last.
- Loading a project holds out-of-range values (for example a speed of 0 or a 1000 °C
  nozzle) to the allowed range.
- Saved files are flushed to disk before they replace the old file, and a failed save
  leaves no temporary file behind.
- The USB connection updates the window a few times a second instead of after every
  line from the printer.
- Slicing is faster: a 79,600-triangle model went from 0.39 s to 0.24 s.
- The printer page no longer copies the whole printer state, or the latest camera
  picture, on every frame.
- Long functions in the slicer, G-code writer, 3D view and screens are split into named
  steps; shared code (atomic saves, base64, bounding boxes, printer details in files,
  HTTP clients, object actions, layout widgets) lives in one place each.

### Fixed
- Crashes on a damaged project file, a 3MF file with non-English characters in an
  attribute, an empty polygon, and deleting or replacing a model during a drag.
- Models with NaN or infinite coordinates are rejected instead of making the slicer
  loop forever.
- An OBJ face with a bad corner is skipped instead of becoming a different triangle; a
  bad corner in a text STL drops only its own facet instead of shifting every later
  triangle.
- Filament position (E) is kept at full precision, so long prints no longer drift.
- A slice that was refused no longer triggers a "Start printing?" prompt later; New,
  Open and Clear plate stop a slice in progress.
- A model under the mouse is no longer picked from anywhere on screen because of one
  zero-area triangle.
- The rotate rings no longer draw a stray line when part of them is behind the camera.
- A Creality print start is no longer lost after a long upload, and the upload no longer
  causes a false "Connection lost".
- A PrusaLink upload no longer reuses a sign-in counter.
- Stopping a USB print can no longer replay old moves.
- Printers reachable only on a later address of their name (such as IPv4 behind an
  IPv6-first `localhost`) now connect.
- The printer search spinner stops if the search fails, and the Records tab shows a
  print once even when the printer also lists it.
- A comment mentioning `BED_MESH_CALIBRATE` no longer counts as already levelling;
  absurd temperatures or layer numbers in a file no longer overflow.
- Command line: the help lists the `aligned` seam; an option is no longer taken as the
  value of the one before it; the Digest sign-in scheme is matched in any case.

### Security
- The zip library used to read 3MF files is upgraded from 2.6 to 9.6, clearing six
  RustSec advisories (RUSTSEC-2026-0367, -0368, -0369, -0429, -0545 and -0548).
  PrintCraft only reads entries by name and was not known to be affected. (73a26c8)
- A long PrusaLink password is no longer sent in plain text as an API key on the first
  request. The key is only tried when the printer offers no Digest sign-in.

## [0.0.1] - 2026-11-17

### Added
- First version of the PrintCraft desktop slicer: STL, OBJ and 3MF import; move, scale,
  rotate, arrange and lay flat; quality presets, walls, infill patterns, supports,
  skirt and brim, ironing, fuzzy skin and vase mode; layer preview with estimates;
  G-code export with thumbnails.
- Printer connections: Creality network service, Klipper (Moonraker), PrusaLink and
  USB, with network search, known printers, live status, controls and camera view.
- Projects (`.printcraft`) that save the plate, settings and printer.
- Command-line slicing (`printcraft --slice`).

[Unreleased]: https://github.com/thawkins/PrintCraft/compare/v0.0.4...HEAD
[0.0.4]: https://github.com/thawkins/PrintCraft/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/thawkins/PrintCraft/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/thawkins/PrintCraft/compare/4a62057...v0.0.2
[0.0.1]: https://github.com/thawkins/PrintCraft/commit/4a62057
