# Handoff

Round: Repetier-Firmware support over USB. Sources: the RepRap wiki pages "Repetier-Firmware" and "G-code", captured as shots/repetier_wiki and shots/gcode_wiki (evidence ev_35f2…, ev_adf5…).
- printer/mod.rs: Firmware enum {Marlin, Klipper, Repetier} with from_name and level_command (G29 / BED_MESH_CALIBRATE / G32). New firmware_name(M115 line) parser. New PrinterStatus.firmware.
- calibration.rs: calibration_lines and with_calibration take a Firmware; added REPETIER_LEVEL "G32".
- device_ui:
  - firmware() is Klipper when the API is Klipper, otherwise the detected status.firmware.
  - send_job uses it.
  - The USB note now says "Marlin, Prusa or Repetier".
- usb.rs:
  - The session sends M115 first; Notice parsing (Firmware / Restarted "start" / Fatal / Error).
  - Streamer has a firmware field; start(gcode, name).
  - Repetier: M530 S1 L / M531 name (cleaned, 40 chars) / M532 X.x L on each 0.1% or new layer / M530 S0 at the end and on stop; no M155; M105 every 3 s while printing.
  - All firmwares: resend forms rs:N, N:, and "wait" clears waiting_ok.
- Tests: 5 new usb tests, calibration repetier_printers_get_g32, and an end-to-end test (printer/tests.rs) with a pretend Repetier printer on a TTYPort::pair pty. nextest 153 pass.
- Docs: PRINTER_LINK.md 3b/3c/6/7 and FEATURES.md.
- Unknown recorded: real Repetier behaviour. Repetier-Server (network) is not supported.
- Clean room kept: the protected AppImage was never read; only public reprap.org pages.
