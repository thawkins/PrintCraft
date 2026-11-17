# PrintCraft

PrintCraft is a simpler desktop slicer for FDM 3D printers, written in Rust with
[egui/eframe](https://github.com/emilk/egui). It turns STL, OBJ and 3MF models into
G-code, shows a layer-by-layer preview, and sends the job to a printer over the network
or a USB cable, then lets you watch and control the print.

It is a clean-room project: it is built from written requirements and public protocol
documentation only, not from any other slicer's code. It has no affiliation with any
printer or slicer maker.

## Features

**Preparing models**
- Import STL (binary and text), OBJ and 3MF. 3MF files from other slicers that keep each
  object in its own part are supported.
- Move, scale and rotate with on-screen handles; arrange, duplicate, lay flat, turn 90°.
- Projects (`.printcraft`) save the plate, every setting and the printer in use. Passwords
  are never saved. The File menu lists the 5 most recent projects.

**Slicing**
- Fine / Standard / Draft quality presets, plus layer height, walls, top and bottom
  layers, infill density and speeds.
- Infill patterns: lines, grid, triangles. Seam position: aligned, nearest, rear, random.
- Supports (optionally only on the plate), skirt, brim or raft, z-hop, and retraction
  with a settable travel threshold, an optional retraction on layer changes, and wipe.
- Ironing, fuzzy skin, a textured top surface and spiral vase mode.
- Cooling slowdown for short layers and fan control.
- Time, filament, weight and cost estimates, and a thumbnail picture in the G-code.

**Printers**
- Printer presets: Prusa MK4S, Core One, MK3S+, MINI+, XL (one tool), Creality
  Ender-3 V3 KE, FlashForge Creator Pro (Sailfish, two nozzles), and four generic plate
  sizes.
- Network printing to Creality's network service, Klipper (Moonraker) and PrusaLink.
- USB printing to Marlin, Prusa and Repetier firmware, with line numbers, checksums and
  resend. The printer page shows which protocol is in use ("Repetier-Firmware v1" or
  "v2", or "Marlin-style protocol"), and every line sent and received is recorded in
  `usb-serial.log` in the settings folder's `logs` folder, for diagnosing a printer.
- MakerBot-protocol printers over USB (Sailfish: FlashForge Creator Pro, early Creator
  Pro 2, Replicator): printing as X3G, with pause, stop, the controls and the SD card's
  files. X3G export saves files for the card (preset "FlashForge Creator Pro
  (Sailfish)").
- Two-nozzle printers (two nozzles on one carriage):
  * a nozzle for each part, a support nozzle and a second filament;
  * a settable primary nozzle (right, T0, by default) and nozzle spacing;
  * one tool change per layer, with the idle nozzle pulled back and kept at a standby
    temperature;
  * a prime tower (its position can be set) and an optional ooze shield;
  * per-nozzle filament and cost, and a preview coloured by nozzle;
  * Ditto printing, with a check that the job leaves room for the copy.

  The plan and the protocol notes are in [makerbot.md](makerbot.md).
- Finds printers on the local network and remembers known ones, and finds a known
  printer again when it has moved to a new address.
- Live status, temperatures, pause / resume / stop, camera view (Creality and Klipper
  MJPEG cameras), and optional bed levelling before a print.

[FEATURES.md](FEATURES.md) lists what is
supported and what is not yet.

## Building and running

This folder holds the Rust project. You need a recent Rust toolchain; run these from
here.

```bash
cargo run --release
```

Open models straight away by naming them:

```bash
cargo run --release -- model.stl other.3mf
```

Connect straight away to a network printer or a USB port:

```bash
cargo run --release -- --connect 192.168.1.50
```

```bash
cargo run --release -- --connect /dev/ttyUSB0
```

Slice without opening a window:

```bash
cargo run --release -- --slice model.stl --output model.gcode --quality fine --filament petg
```

For a MakerBot / Sailfish printer (FlashForge Creator Pro, early Creator Pro 2), write an
X3G file instead:

```bash
cargo run --release -- --slice model.stl --output model.x3g --printer 11
```

`printcraft --help` lists every option (printer preset, supports, brim, infill pattern,
seam, ironing, vase mode, fuzzy skin and more).

## Development

Tests run with [cargo-nextest](https://nexte.st), which runs each test in its own
process. Install it once with `cargo install cargo-nextest --locked`, then:

```bash
cargo nextest run
```

Settings are in `.config/nextest.toml`; `cargo nextest run --profile ci` also retries a
failure once and writes a JUnit report to `target/nextest/ci/junit.xml`. For line
coverage, `cargo llvm-cov nextest` works with the same tests.

```bash
cargo fmt --check
```

```bash
cargo clippy --all-targets
```

Further documentation:
- [MECHANICS.md](MECHANICS.md): how slicing
  works and every number it uses.
- [PRINTER_LINK.md](PRINTER_LINK.md): how
  each printer connection works.
- [docs/REFACTOR_AUDIT.md](docs/REFACTOR_AUDIT.md):
  the latest code audit and its open items.
- [makerbot.md](makerbot.md): the MakerBot / Sailfish and two-nozzle plan, its sources
  and what is done.
- [BUILDSTATS.md](BUILDSTATS.md): measured build times, disk use, line coverage and
  test totals.
- [AGENTS.md](AGENTS.md) and [AGENTS-LANGUAGE.md](AGENTS-LANGUAGE.md): instructions for
  coding agents working in this folder (copied from the ragent project).
- [CHANGELOG.md](CHANGELOG.md): notable changes.

## License

MIT. See [LICENSE](../../LICENSE).
