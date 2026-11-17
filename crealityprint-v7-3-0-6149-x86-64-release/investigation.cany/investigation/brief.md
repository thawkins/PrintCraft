# Brief

**Question:** Build a simpler desktop 3D-printing slicer with the same core features as the target (load models, place on a build plate, choose printer/filament/quality, slice, preview layers, export G-code).

**Purpose:** A simpler replacement app for a person who doesn't code

**Targets:**
- /home/thawkins/bin/CrealityPrint-V7.3.0.6149-x86_64-Release.AppImage

**Constraints:**
- Clean room per project AGENTS.md: never inspect, extract or decompile the AppImage; build only from the written request and reference/ (empty)
- Rust + egui/eframe desktop app in app/
- Must support --screenshot <file.png>

**Done when:** App builds, opens a window with a 3D plate view, loads STL, slices into layers, previews, exports G-code, and the screenshot option saves a non-blank PNG.
