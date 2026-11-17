# Changelog

All notable changes to PrintCraft are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- 18 more tests (139 in all), covering geometry, presets, saved printers, the slicing
  pipeline, Klipper status, USB parsing, camera frames, the 3D view and base64.
- Tests run with cargo-nextest (`cargo nextest run`), configured in
  `.config/nextest.toml`, with a `ci` profile that retries once and writes JUnit.
- Opening a project saved by a newer PrintCraft warns that it may use features this
  version doesn't support, and that saving it here drops them.
- 3MF files in inches, centimetres, metres, feet or microns now load at the right size
  (the `unit` of the model was ignored before).
- Setting ranges are defined once and shared by the settings panel and the project
  loader.
- A project's "Save As" adds `.printcraft` to the name instead of replacing what follows
  a dot.

### Changed
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

[Unreleased]: https://github.com/thawkins/PrintCraft/compare/4a62057...HEAD
[0.0.1]: https://github.com/thawkins/PrintCraft/commit/4a62057
