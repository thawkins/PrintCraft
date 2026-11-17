# PrintCraft

A simpler desktop slicer for FDM 3D printers, written in Rust with egui/eframe. It
slices STL, OBJ and 3MF models and sends the print to Creality, Klipper (Moonraker),
PrusaLink or USB printers.

The application lives in
[`crealityprint-v7-3-0-6149-x86-64-release/app/`](crealityprint-v7-3-0-6149-x86-64-release/app/):
see its [README](crealityprint-v7-3-0-6149-x86-64-release/app/README.md) for features
and how to build and run it, and its
[CHANGELOG](crealityprint-v7-3-0-6149-x86-64-release/app/CHANGELOG.md) for changes.

## Quick start

From the repository root, move to the app folder:

```bash
cd crealityprint-v7-3-0-6149-x86-64-release/app
```

Then build and start PrintCraft (needs a recent Rust toolchain):

```bash
cargo run --release
```

To open models straight away, name them after `--`:

```bash
cargo run --release -- model.stl
```

MIT licensed; see [LICENSE](LICENSE).
