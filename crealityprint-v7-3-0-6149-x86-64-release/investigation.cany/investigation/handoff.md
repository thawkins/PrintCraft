# Handoff

Round: maximise button on the enlarged camera window.
- device_ui camera_window:
  - Custom title bar (title_bar(false)) with drawn window_button icons: Maximise (square), Restore (two squares) and Close (cross; red when hovered).
  - Maximised uses a separate window id "camera_max" with fixed_rect(content_rect.shrink(6)), not movable or resizable; restore returns to the "camera_window" id's own size and place.
  - Double-clicking the title toggles; Esc restores.
  - New fields camera_max and camera_wanted (opens the camera once its first frame arrives); show_camera_when_ready().
- New option --camera big|max (main.rs, StartOptions.camera).
- New test escape_restores_a_maximised_camera_and_closing_resets_it.
- Fixed a flaky Repetier USB test: the pretend board no longer closes its pty right after the last line; it passed 6 runs in a row.
- Checks: nextest 156 pass. Clippy shows only the 3 old warnings.
- Screenshots: shots/camera_big.png and shots/camera_max.png, taken against shots/fake/fake_printer.py (127.0.0.1:7125 and :8080, runs 40 s).
- Docs: PRINTER_LINK.md 4a Camera line.
- Clean room kept.
