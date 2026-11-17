# Changelog

All notable changes to PrintCraft are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/thawkins/PrintCraft/compare/v0.0.2...HEAD
[0.0.2]: https://github.com/thawkins/PrintCraft/compare/4a62057...v0.0.2
[0.0.1]: https://github.com/thawkins/PrintCraft/commit/4a62057
