# Driving printers: Wi-Fi (Creality, Klipper, Prusa) and USB

This is a clean-room investigation. Nothing here comes from the original slicer. The
original program was not opened, extracted or decompiled, as the project's AGENTS.md
requires. The facts come from:
* public, community-documented behaviour of Creality's Klipper-based firmware
  ("Creality OS")
* Moonraker's official API documentation
* Prusa's published PrusaLink API
* the RepRap/Marlin serial host protocol

**Confidence labels**
* **documented**: the maker's published API.
* **community**: widely reported by owners and open-source integrations.
* **confirmed**: tried by the person on their own printer.
* **unverified**: not yet tried against a real printer.

**Confirmed on the person's Ender-3 V3 KE (2026-11-15):** upload over the network works,
the camera video works, and the print job can be watched while printing. They also
reported the screen showing "Layer 9 of 0". Section 4 describes the fix.

## 0. Known printers (src/printer/known.rs)

Every printer a Wi-Fi search finds, and every printer the app connects to (address,
USB port or typed-in address), is saved in a **Known printers** list. When the
connection works, the entry takes the printer's own name, its model and the kind of
connection. The list shows first on the Device page, newest first, with **Connect** and
**Forget** buttons.
* It is stored as JSON in the user's settings folder: Linux
  `~/.config/printcraft/known_printers.json` (or `$XDG_CONFIG_HOME`), macOS
  `~/Library/Application Support/printcraft/`, Windows `%APPDATA%\printcraft\`.
* Each entry holds the address or port, name, model, kind of connection, USB speed and
  the last time it was used.
* **PrusaLink passwords are never saved**; type the password again before connecting.
* The file is written through a temporary file, so it is never left half-written.
* While the app takes a check picture (`--screenshot`) it may show the list but never
  changes it.
* `printcraft --device` opens straight on the Device page.

### A known printer with a new address (person's request; `discovery::relocate`)

Routers often give a printer a new address, for example after it restarts. When **Connect**
is pressed on a known network printer, or a project with a printer is opened, the app
looks for the printer before connecting. "Looking for <name>…" shows while it does.
1. **The saved address.** It is asked on the printer kind's port (Creality network
   service 9999, Moonraker 7125, PrusaLink 80), with 1.5 s to answer. If the printer there
   gives the same name (its hostname, e.g. `Ender3V3KE-A654`), or no name, that is it.
2. **Otherwise the network is searched.** Every address of this computer's local
   network (x.x.x.1-254) is asked on that one port, 64 at a time with 0.4 s each. Each
   that answers is asked its name, and the printer of the same kind and name is used.
   This covers both the old address not answering and a different printer now having
   it.
3. **Found at a new address.** The app connects there. The known list replaces the old
   entry with the new address, so the printer isn't listed twice, and the Activity log
   says "<name> has a new address: … (it was …)".
4. **Not found.** An alert says the printer didn't answer at its saved address and that
   a search of the local network didn't find it either. From a project, no printer is
   selected.

A printer saved without a name of its own ("Printer at …") can't be recognised
elsewhere, so only its saved address is tried. USB printers are only looked for on
their port. A ping isn't used, because many printers and networks don't answer pings
while the printer's own port does.

## 1. Finding the printer (src/printer/discovery.rs)

* The printer joins your Wi-Fi and gets an address. Its screen shows the address in the
  network settings.
* **Find printers on Wi-Fi** asks every address in this computer's local network
  (x.x.x.1–254). It checks for port **9999** (Creality's service), **7125** (Moonraker)
  or **80** (PrusaLink), with a 0.4 s limit per address and 64 addresses at a time.
* Each hit is identified. On port 9999, the app listens for up to 2 s for the printer's
  `hostname` and `model`. On port 7125, it reads `GET /printer/info`. On port 80, it
  reads `GET /api/version`. Only answers that say "PrusaLink", or that refuse with
  PrusaLink's `Printer API` sign-in, count as PrusaLink. Otherwise an answer whose text
  says "OctoPrint", or (refused without a key) a front page that says "OctoPrint",
  counts as an OctoPrint server. Routers and other web devices are skipped.
* You can also type the address by hand, or start the app with `--connect ADDRESS`.
* Not used: mDNS/Bonjour discovery. Whether stock KE firmware announces itself that way is
  unverified, and the address scan works without it.

## 2. Stock firmware: Creality network service (src/printer/creality.rs) — community; upload, status and camera confirmed on a KE

| What | How |
|---|---|
| Live status | WebSocket `ws://PRINTER:9999`. The printer pushes JSON messages that carry only the fields that changed. |
| Keep-alive | Every 10 s the app sends `{"ModeCode":"heart_beat","msg":"<time>"}`. If the printer sends a heartbeat, the app answers `ok`. 45 s of silence means the connection is lost, so the app reconnects every 3 s. |
| Upload a file | HTTP `POST http://PRINTER/upload/<name>.gcode`, multipart field `file`. It lands in `/usr/data/printer_data/gcodes/`. |
| Start printing | `{"method":"set","params":{"opGcodeFile":"printprt:/usr/data/printer_data/gcodes/<name>.gcode"}}` |
| Pause / resume | `{"method":"set","params":{"pause":1}}` / `{"pause":0}` |
| Stop | `{"method":"set","params":{"stop":1}}` |

Status fields read: `state` (0 ready, 1 printing, 2 finished, 3 failed, 4 stopped,
5 paused), `printProgress` (0–100), `printJobTime` (s), `printLeftTime` (s), `layer`,
`TotalLayer`, `printFileName`, `nozzleTemp`, `targetNozzleTemp`, `bedTemp0`,
`targetBedTemp0`, `hostname` and `model`. Numbers may arrive as text, such as
`"25.120000"`; both forms are accepted.

## 3. Rooted printers or Klipper add-ons: Moonraker (src/printer/moonraker.rs) — documented

| What | How |
|---|---|
| Identify | `GET :7125/printer/info` |
| Live status (polled every 1 s) | `GET :7125/printer/objects/query?print_stats&virtual_sdcard&extruder&heater_bed&display_status&fan&gcode_move&exclude_object` |
| Upload and start | `POST :7125/server/files/upload`, multipart `file` plus `print=true` |
| Pause / resume / stop | `POST :7125/printer/print/pause`, `/resume`, `/cancel` |
| Objects of the print | `exclude_object.objects` (names) and `exclude_object.excluded_objects` in the status. Present only when Klipper has `[exclude_object]` and the file names its objects ("Label objects") |
| Cancel one object | `EXCLUDE_OBJECT NAME=<name>` via `POST /printer/gcode/script` |

Time left = elapsed ÷ progress − elapsed. This is the same simple projection most
Klipper web pages use. The layer numbers come from `print_stats.info`, which only appears
when the print file sets layer information.

## 3a. Prusa printers: PrusaLink (src/printer/prusalink.rs) — documented, unverified on a real printer

PrusaLink is built into the MK4/MK4S, Core One, MINI/MINI+ and XL. The MK3S gets it with a
Raspberry Pi. Prusa publishes it as an OpenAPI description (API v1).

| What | How |
|---|---|
| Sign-in | HTTP **Digest**, user `maker` plus the password shown on the printer (Settings → Network → PrusaLink). Type it in the **PrusaLink password** box before connecting. A long password without spaces is also sent as an `X-Api-Key`, for older PrusaLink versions. |
| Identify | `GET /api/version`, which answers "PrusaLink" or asks for the `Printer API` sign-in |
| Status (every 2 s) | `GET /api/v1/status`: `printer.state`, `temp_nozzle`/`target_nozzle`, `temp_bed`/`target_bed`, `job.id`, `job.progress`, `job.time_printing`, `job.time_remaining` |
| File name | `GET /api/v1/job`: `file.display_name` |
| Upload and start | `GET /api/v1/storage` picks the first writable storage (`usb` on Buddy printers, `local` on a Pi). Then `PUT /api/v1/files/<storage>/<name>.gcode` with `Print-After-Upload: ?1` and `Overwrite: ?1`. |
| Pause / resume / stop | `PUT /api/v1/job/<id>/pause`, `PUT /api/v1/job/<id>/resume`, `DELETE /api/v1/job/<id>` |
| Cancel one object | Not offered by the API (no object list or cancel call). The printer's own Cancel object menu uses the M486 labels in the file |
| Binary G-code | With "Binary G-code (.bgcode)" on in the printer profile: the same `PUT`, to `<name>.bgcode`, with `Content-Type: application/octet-stream` |

States: `IDLE`/`READY`/`BUSY` show as Ready, plus `PRINTING`, `PAUSED`, `FINISHED`,
`STOPPED`, and `ERROR`/`ATTENTION` (shown as Failed). PrusaLink doesn't report layers or
a local camera stream, so those stay empty for Prusa printers. Prusa cameras send their
pictures to the Prusa Connect cloud service, which isn't supported here.

Prusa printers prefer Prusa's own start code and binary G-code (`.bgcode`). PrintCraft
sends plain `.gcode`, which PrusaLink also accepts, or `.bgcode` when the printer
profile has "Binary G-code" on. Plain G-code from another slicer may
show a "printer model mismatch" warning on the screen; it can be confirmed.

## 3a2. OctoPrint (src/printer/octoprint.rs) — documented, unverified on a real server

OctoPrint runs on a Raspberry Pi (OctoPi) next to a printer on a USB cable. Its REST API
is documented by the OctoPrint project. Sign-in is an API key (OctoPrint's settings,
"Application Keys"), typed into the "Password or API key" box and sent as `X-Api-Key`. It
is never saved.

| What | How |
|---|---|
| Identify | `GET /api/version` (text "OctoPrint ..."), or the front page when that is refused |
| Live status (polled every 2 s) | `GET /api/printer` (temperatures, state flags; 409 = not connected to the printer) and `GET /api/job` (file, completion, print time, time left) |
| Upload and start | `POST /api/files/local`, multipart `file` plus `select=true` and `print=true` |
| Print a stored file | `POST /api/files/local/<path>` with `{"command": "select", "print": true}` |
| Pause / resume / stop | `POST /api/job`: `{"command": "pause", "action": "pause"}` or `"resume"`; `{"command": "cancel"}` |
| Temperatures | `POST /api/printer/tool` `{"command": "target", "targets": {"tool0": T}}`; `POST /api/printer/bed` `{"command": "target", "target": T}` |
| Fan, speed, jog, home | G-code through `POST /api/printer/command` `{"commands": [...]}` |
| Files | `GET /api/files?recursive=true` (G-code in every folder, newest first) |
| Camera | OctoPi's MJPEG stream, `:80/webcam/?action=stream` (one of the addresses the camera view tries) |

States from the flags: error or closedOrError = Failed, cancelling = Stopped, paused or
pausing = Paused, printing = Printing, operational = Ready. OctoPrint has no print
history and no layer numbers in its API. A wrong key shows "OctoPrint refused the API
key"; binary G-code and cancel object aren't offered.

## 3b. USB cable (src/printer/usb.rs) — standard RepRap host protocol, unverified on a real printer

For printers running **Marlin**, **Prusa firmware** (MK3S, MINI, MK4 and most older
printers) or **Repetier-Firmware** (see 3c). **Klipper printers such as the Ender-3 V3 KE can't be driven over USB this
way**: their USB socket leads to Klipper's own controller. Use the network for them.

* The port list comes from the system's serial devices (`/dev/ttyACM*`, `/dev/ttyUSB*`,
  `COM` ports). The speed is 115200 baud by default (250000, 230400, 57600 and 500000 can
  be chosen).
* Opening the port restarts many boards, so the app waits 2 s first. Then it asks
  `M115`. The `FIRMWARE_NAME:` in the answer is shown on the Device page as the printer's
  model, and decides how the printer is driven (see 3c for Repetier).
* Starting a print sends `M110 N0` (line numbers restart), then `M155 S2` (report
  temperatures every 2 s), then each command of the file without comments, as
  `N<line> <command>*<checksum>` (XOR of every character before `*`).
* Exactly one line is in flight: the next goes out after the printer answers `ok` (also
  `ok 12`). `Resend: N`, `Resend: N12`, `rs N` or `rs:N` sends from line N again.
  `wait` (the printer's buffer is empty) means an `ok` was lost, so the app carries on.
  `busy:` keeps it waiting. If the printer is silent for 60 s, the app carries on.
* `start` (the board restarted) during a print ends the job: the printer has lost it.
  `!!`, `Error:` and `fatal:` lines are shown to the person, and `fatal:` (the firmware
  has shut down) ends the job. These replies are from the "Replies from the RepRap machine"
  part of the RepRap wiki's G-code page.
* Temperatures are read from `T:210.0 /210.0 B:60.0 /60.0`. When idle, the app asks with
  `M105` every 3 s.
* Progress is lines sent ÷ lines in the file. The layer comes from the file's `;LAYER:`
  markers. Time left = elapsed × (100 − progress) ÷ progress.
* **Pause** stops sending; the printer finishes the line in hand and waits. **Stop**
  drops the rest and sends `M104 S0`, `M140 S0`, `M107` and `M84` (heaters, fan and
  motors off).
* The computer must stay awake with the app open until the print finishes. Closing the
  app or the Printer connection turns the heaters off.

### Serial log (src/printer/serial_log.rs)

Every USB connection records what is said over the port, to diagnose a printer that
misbehaves. The file is `usb-serial.log` in the `logs` folder of the settings folder
(Linux `~/.config/printcraft/logs/`, macOS `~/Library/Application Support/printcraft/logs/`,
Windows `%APPDATA%\printcraft\logs\`). The activity log names it when the port opens.

* Each line has the time of day, the seconds since the port opened, and `>>>` for what
  the app sent, `<<<` for what the printer answered, or `---` for the app's own notes
  (such as why the connection ended).
* A new connection moves the last log to `usb-serial.previous.log`, so a printer that
  restarts and reconnects keeps the evidence of what went wrong.
* The file stops at 50 MB. Only G-code and the printer's replies go over the port, so
  nothing secret is recorded.
* Tests that drive pretend printers write to a temporary folder instead.

## 3c. Repetier-Firmware over USB (src/printer/usb.rs) — documented, unverified on a real printer

Sources: the RepRap wiki pages "Repetier-Firmware" and "G-code", read for this.

* **Detection.** Repetier-Firmware speaks the same numbered, checksummed ASCII lines. It
  works out by itself whether a host uses ASCII or its own binary protocol. When M115 names
  Repetier, the app switches to the steps below. Choose the board's speed under USB;
  250000 is in the list.
* **Shown to the person.** A USB connection's header has a badge with the protocol in
  use: "Repetier-Firmware v1" (the 1.x releases, and the 0.9x releases they grew from)
  or "Repetier-Firmware v2" (2.x), from the version in the M115 name, or "Marlin-style
  protocol". Hovering it shows the name the printer reported. The activity log says the
  same when the printer names itself. Both versions are driven the same way so far.
* **Print mode on the printer's screen.** Starting a print sends `M110 N0`, then
  `M530 S1 L<layers>` ("a print has started", with the layer count) and `M531 <name>`.
  Comment and checksum marks are removed from the name, and it is cut to 40 characters.
  Marlin's `M155` is not sent.
* **Progress.** `M532 X<percent> L<layer>` goes out whenever the progress has moved by
  0.1 % or a new layer has begun. The wiki says "every 0.1% progress change on every
  layer change".
* **End.** `M530 S0` goes out once after the last line, and before the heaters-off lines
  when the print is stopped.
* **Temperatures** are asked for with `M105` every 3 s during a print as well, because
  the app doesn't rely on automatic reports.
* **Calibrate before printing** uses `G32`, Repetier's "Probe Z and calculate Z plane",
  with the printer's own configured method. In Repetier, `G29` only measures the height.
* **Heat waits.** `M109 S` and `M190 S` wait only while heating, as in Marlin, so print
  files need no change.

## 3d. MakerBot protocol / Sailfish over USB (src/printer/sailfish.rs, src/x3g) — documented, unverified on a real printer

For Sailfish and MakerBot firmware: the FlashForge Creator Pro and early Creator Pro 2,
Replicator 1/2/2X and their clones. The plan and the sources are in makerbot.md.

Sources:
* MakerBot's protocol specification (`doc/s3gProtocol.md` in the `makerbot/s3g`
  repository);
* the Sailfish 7.8 source (`Packet.hh`, `Packet.cc`, `Commands.hh`, `Host.cc`,
  `EepromMap.hh`, `Configuration.hh`).

* **Choosing it.** Under USB, set **Protocol** to "MakerBot / Sailfish (X3G)". It
  isn't detected by itself yet. The choice is saved with the printer (`"api": "sailfish"`).
* **Packets.** Each packet is:
  * `0xD5`;
  * the length;
  * the payload (at most 32 bytes);
  * an iButton/Maxim CRC-8 of the payload.

  One question is in flight at a time. A damaged, missing or "send again" answer
  (`0x80`, `0x82`, `0x83`, `0x88`, `0x8C`) is asked again, up to 5 times. Any other error
  code is final for that question.
* **What it asks on connecting and while idle (nothing that changes the printer):**
  * version (0) and advanced version (27);
  * a copy of the whole 4 KB EEPROM (12, 31 bytes at a time);
  * position (21);
  * then every 2 s: nozzle and target temperatures for each nozzle, and bed and target
    temperatures (10 with subcommands 2, 32, 30, 33);
  * motherboard status (23) and build statistics (24);
  * the build name (20) while a build runs.
* **Never sent:** 3 (clear buffer) and 7 (abort), which make Sailfish restart the board.
  Tests check this for connecting, printing, stopping and the controls.
* **Shown to the person:**
  * a "MakerBot protocol · Sailfish 7.8" badge, from the variant byte: 0x80 is Sailfish
    (the specification and `Configuration.hh`), and 2 is accepted too;
  * the machine's settings, read from the EEPROM copy: its name, nozzle count, heated
    bed, Ditto printing, and for each axis the steps per mm, length, maximum speed and
    home direction;
  * temperatures (right and left nozzle on two-nozzle machines, with tool 0 as the
    right one);
  * a print started from the printer's own SD card, with its name and running time.
* **Printing.** The sliced plate is translated to S3G (`x3g::translate`; the plate
  size comes from the `; plate:` line in the G-code's header).
  * The app first tries to record the job onto the SD card (14, the commands, 15) and
    print it from there (16), so the computer may disconnect.
  * Stock Sailfish is built without that recording (`S3G_CAPTURE_2_SD` is off in every
    board's `Configuration.hh`) and answers "unsupported". So does a card that isn't
    ready, with the reason. In both cases the commands are streamed over USB instead,
    paced by the printer's "buffer full" answers. Progress is how many commands have
    been sent.
  * When an answer to a command is lost, the command is sent again, as GPX does. If
    the printer had in fact taken it, an extrusion move can run twice.
* **Pause / resume** use query 8, which toggles, so it is sent only to change state.
  **Stop** uses 22 (halt the motors and empty the queue), then switches both nozzles and
  the bed off and ends the build. Extended stop leaves Sailfish's build state alone, and
  the build end then reads "finished normally", so the app keeps showing Stopped until
  the next print. Disconnecting stops a streamed print. A print running from the card
  carries on.
* **Controls.**
  * Nozzle and bed temperatures and the part fan (output 13, on/off) are queued
    commands. During a print they take effect after what is already queued.
  * Jog sends a relative move (155 with every axis relative), at 3000 mm/min for X/Y
    and 600 mm/min for Z, capped at the axis's limit.
  * Home sends the same homing as a print (X/Y to max, Z to min, then the stored home
    position).
  * Jog and home are refused while a print runs. The print speed can't be changed:
    S3G has no command for it.
* **Files.** The SD card's `.x3g` / `.s3g` files are listed with 18 and printed with 16.
  There is no print history.
* **Records.** The packets go into `usb-serial.log` as hex, with what each one means. The
  EEPROM copy is saved as `sailfish-eeprom.bin` beside it.

## 4. Progress and monitoring (src/device_ui.rs)

The Printer page shows:
* state and file name
* a progress bar
* layer X of Y
* time printing and time left
* nozzle and plate temperatures, with what they are set to
* Pause, Resume and Stop buttons; Stop asks first
* recent activity

The status bar shows "printing 42 %" on every page. **Send to printer** (toolbar) and
**Print on this printer** (Printer page) always ask for confirmation first ("make sure the
plate is clean and empty"). They are disabled while the printer is busy.

### Calibrate before printing (src/printer/calibration.rs): on by default

The Printer page has a **Calibrate the printer before printing** switch. It also appears
in the "Start printing?" confirmation. It is on when the app starts. When it is on, the
file sent to the printer gets two extra lines right after the plate-heat wait (`M190`):

```
G28 ; home again now the plate is hot
BED_MESH_CALIBRATE ; printer calibration: measure the plate
```

`BED_MESH_CALIBRATE` is Klipper's documented bed-levelling command. The KE runs Klipper,
so the same file works over the Creality service and over Moonraker. Prusa (PrusaLink)
and USB printers get the standard `G29` instead, or `G32` on Repetier-Firmware. The lines are added only once, and only
to files sent from the Printer page; **Export G-code** is unchanged. The switch is not
remembered between app starts.

**Calibrating twice on the KE (person's report):** the KE measures the plate by itself
when a print starts. The app's extra `BED_MESH_CALIBRATE` made it measure a second time.
For printers on the Creality network service, the app no longer adds its own step, and
the switch explains that the printer does it. Moonraker, PrusaLink and USB printers
still get the step when the switch is on.

**Enable Calibration (Device page, on by default; person's request):** a checkbox at
the top of the Device page's Print settings. It also appears in the send card and in
the "Start printing?" and "Print again?" dialogs.
* **Creality network service (KE):** the printer calibrates by itself, so the choice is
  sent to the printer as its `enableSelfTest` setting (`{"method":"set","params":
  {"enableSelfTest":0|1}}`). This happens straight away when the box is changed, and
  again before every print. The start command also carries `"enableSelfTest"` next to
  `opGcodeFile`. When the printer reports its own setting (`enableSelfTest` or
  `withSelfTest` in its status), the page shows "The printer reports: calibration on/off".
  **Community-reported and unverified on the KE**: if it still calibrates with the box
  off, its firmware uses a different name, and the printer's own screen setting decides.
* **Moonraker, PrusaLink and USB printers:** on adds the app's levelling step (see below);
  off leaves it out.

**Nozzle cooling to 0 °C before calibration (person's report):** before levelling, the
printer set the nozzle to 0 °C. It waited for it to cool, then heated it again for the
print. Two changes:
1. **Job temperatures in every print file** (`gcode::header_block`, `gcode::config_block`).
   A printer that prepares (levels) before running a file can only prepare at the job
   temperature if it can find it in the file. PrintCraft's files didn't say, so the
   printer most likely fell back to 0 °C. The job's nozzle and plate temperatures are now
   written in the forms other slicers use:
   * a header block (`; nozzle_temperature_initial_layer = 210`,
     `; bed_temperature_initial_layer_single = 60`, `; filament_type = PLA`, …)
   * Cura-style lines (`;EXTRUDER_TRAIN.0.INITIAL_TEMPERATURE:210`,
     `;BUILD_PLATE.INITIAL_TEMPERATURE:60`)
   * a closing settings block (`; first_layer_temperature`,
     `; first_layer_bed_temperature`, `; hot_plate_temp_initial_layer`, …)
2. **When the app adds the levelling step itself** (Moonraker, PrusaLink and USB
   printers), the step now comes only after the nozzle has reached the job temperature
   (after `M109`). It sets `M104 S<job temp>` just before measuring, and
   `M109 S<job temp>` afterwards, so the nozzle is never left to cool.

Unverified: which of these keys the KE's own preparation reads. If the nozzle still drops
to 0 °C, the KE probably cools it on purpose to keep it clean while probing; no print file
can change that. The next step would be the printer's own "print calibration" setting.

### Pictures of the part on the printer's screen (src/thumbnail.rs)

The person asked how the Creality tool gets a picture of the part onto the printer's
screen. It isn't specific to Creality or to Klipper itself. Slicers embed small PNG
pictures as comments in the G-code header, in the widely used format:

```
;
; thumbnail begin 300x300 27336
; iVBORw0KGgoAAAANSUhEUgAAASwAAAEsCAYAAAB5fY51AAB...   (base64, 78 characters a line)
; thumbnail end
;
```

Moonraker reads these blocks into the file's metadata, so web pages and screens can
show them. Klipper-based Creality printers show them on their screen while printing.
PrintCraft now draws the plate's models itself and writes two pictures into every print
file (export, Send print and the command line): **300×300 and 96×96 PNG**. Those sizes
are the ones commonly set up for these printers; they are **estimated**. The picture is
drawn turned 30° and looking down 35°, flat-shaded, at double size then scaled down,
with a transparent background. The Device page shows the same picture in "Current
printing" for jobs sent from the app.

Unverified on the KE: whether its screen picks up the 300×300 or the 96×96 picture.
Some Creality screens look for other sizes or a JPG variant. If no picture appears, the
sizes can be changed in `thumbnail::SIZES`.

### Layer count ("Layer 9 of 0" fix)

The person's KE showed "Layer 9 of 0", so the printer wasn't getting the total.
Three changes:
1. Every print file now states the total in the header in two common forms:
   `;LAYER_COUNT:N` and `; total layer number: N`.
2. Files sent to Klipper printers (KE and Moonraker) get Klipper's documented
   `SET_PRINT_STATS_INFO TOTAL_LAYER=N` before the first layer, and
   `SET_PRINT_STATS_INFO CURRENT_LAYER=k` at each layer change
   (src/printer/layer_info.rs).
3. If a printer still reports 0, the page uses the layer count of the job sent from this
   app, or shows just "Layer 9".

Unverified: whether the KE's own screen reads the new header line or the Klipper
command. The Printer page in this app shows the right total either way.

Unverified: if the KE's own start macro already levels the bed, this adds a second
measurement. Also, Creality's stock service may have its own "calibrate" setting in the
start command. It isn't used because its exact name isn't publicly confirmed.

## 4a. Device page controls, matching the printer screen in reference/picture-1.png

The person shared a screenshot of the original's Device page for their KE. The page now
has the same parts. The look is PrintCraft's own; nothing from the original program
itself was used.

* **Printer tab:** `name_address` with a connection dot and a close (disconnect) button.
  Disconnecting does not cancel an upload already in progress: the file finishes
  sending and, for upload-and-print, the print still starts (decided 2026-11-17).
* **Camera:** Refresh (reconnect the video) and Enlarge (a large window; clicking the
  picture also enlarges it). The enlarged window's title bar has **Maximise** (a square)
  and Close. Maximised, the camera fills the app's window and the button becomes
  **Restore** (two squares), which returns it to its earlier size and place.
  Double-clicking the title bar switches between the two, and Esc restores.
  `--connect ADDRESS --camera big|max` opens it either way for check pictures.
* **Current printing:**
  * a thumbnail (a plain file-and-cube picture, since the printer's own thumbnail isn't
    available)
  * the file name, time printing as `00h 37m`, and the layer
  * the state, plus *Estimated completion time* as the local clock time (`11/16 01:10` =
    now + time left). It uses the computer's own time zone even when the app is started
    with `TZ=UTC`, which some launchers do. A plain UTC setting is dropped at start-up
    when the system has its own zone (`use_system_time_zone`); other TZ settings are
    kept. Hovering the time shows the zone, e.g. "UTC+07:00".
  * a progress bar
  * **Print again**, **Pause/Resume** and **Stop** (Stop asks first)
* **Print settings:** Nozzle and Bed temperature (current plus an editable target, sent
  once you finish typing or dragging), the Model fan switch and wind-speed slider, and
  Printing speed: Silent, Stable 50%, Standard 100%, Ultrafast 125%. The screen shows no
  number for Silent; **50 % is an estimate**.
* **Tabs:**
  * **XYZ Control:** 1 / 10 / 100 mm steps; Y+, Y−, X−, X+, Z+, Z− and Home. Locked
    while printing.
  * **Local Files:** files on the printer, each with a Print button (asks first).
  * **Records:** the printer's own history where it offers one, plus prints this app saw
    end.
  * **Timelapse Video:** not available (see below).

How each connection carries the controls:

| Control | Creality service (community, **unverified**) | Moonraker (documented) | PrusaLink | USB |
|---|---|---|---|---|
| Nozzle target | `{"method":"set","params":{"nozzleTempControl":T}}` | `M104 S<T>` via `POST /printer/gcode/script` | not offered | `M104 S<T>` |
| Bed target | `{"bedTempControl":{"num":0,"val":T}}` | `M140 S<T>` | not offered | `M140 S<T>` |
| Model fan | `gcodeCmd` `M106 P0 S<0-255>` / `M107` | same G-code | not offered | same G-code |
| Speed | `gcodeCmd` `M220 S<%>` | `M220 S<%>` | not offered | `M220 S<%>` |
| Move / home | `gcodeCmd` `G91` / `G1 X10 F3000` / `G90`, `G28` | same G-code | not offered | same G-code |
| Files | `{"method":"get","params":{"reqGcodeFile":1}}`, read loosely | `GET /server/files/list?root=gcodes` | `GET /api/v1/files/<storage>` | none |
| Print a stored file | `opGcodeFile` `printprt:<path>` | `POST /printer/print/start?filename=` | `POST /api/v1/files/<storage>/<name>` | none |
| Records | `{"method":"get","params":{"reqHistory":1}}`, read loosely | `GET /server/history/list` | this app's own only | this app's own only |
| Fan % / speed % shown | `modelFanPct`, `curFeedratePct` | `fan.speed`, `gcode_move.speed_factor` | – | what was last set |

XY moves run at 3000 mm/min and Z at 600 mm/min (estimated).

**Timelapse videos:** how the KE shares its timelapse files isn't publicly documented, so
the tab says so instead of guessing.

### "Print again?" (src/device_ui.rs `observe`, `again_dialog`)

When the printer's state goes from printing or paused to **finished**, a "Print again?"
dialog appears on whatever page is showing. It has **Yes** and **No** (and the
calibration switch when the plate is sent again).
* **Yes** sends the same sliced plate again, if it was sent from this app. Otherwise it
  restarts the same file from the printer's storage (not over USB).
* A stopped or failed print is recorded but doesn't ask.
* Connecting to a printer that has already finished doesn't ask either.
* The same choice is available any time with the **Print again** button.

## 5. Camera (src/printer/camera.rs) — community, unverified for the KE

The optional Creality camera and the usual Klipper set-ups serve **MJPEG**: one endless
HTTP response of JPEG pictures. The app tries these addresses in order and uses the first
that answers with `multipart/x-mixed-replace`:

1. `http://PRINTER:8080/?action=stream` (stock Creality camera service)
2. `http://PRINTER/webcam/?action=stream`
3. `http://PRINTER:4408/webcam/?action=stream` (Fluidd)
4. `http://PRINTER:4409/webcam/?action=stream` (Mainsail)

The app cuts pictures out of the stream at the JPEG start and end markers, decodes them,
and shows the newest one. If no camera is found, the page says so and offers
**Try again**.

## 6. How this was tested

* Unit tests check the status parsing for both protocols, the command messages,
  multipart bodies, file-name cleaning and splitting the MJPEG stream.
* End-to-end tests run a pretend Creality printer (a WebSocket on 127.0.0.1:9999) and
  a pretend Klipper printer (HTTP on 127.0.0.1:7125). They check that status arrives,
  that Pause reaches the printer, and that an upload arrives with the file and
  `print=true`.
* A screenshot was taken connected to a pretend Klipper printer with a pretend MJPEG
  camera on this computer only.
* An end-to-end test runs a pretend PrusaLink printer that insists on Digest sign-in. It
  checks the status, the file name, upload-and-print to `/api/v1/files/usb/…` with
  `Print-After-Upload`, and pause.
* USB: the line protocol is tested with a pretend printer. The tests cover numbering,
  checksums (checked by hand), a damaged line being sent again, pause, and stop turning
  the heaters off. No real serial printer was attached.
* Repetier: an end-to-end test drives a pretend Repetier printer on a pseudo serial port
  (Linux/macOS). It checks that M115 names the firmware, the print-mode, name and progress
  commands, the line checksums, recovery from `rs:2` followed only by `wait`, and that the
  job is reported finished.
* The digest sign-in is checked against the worked example in RFC 2617.

## 7. Still unknown

* KE: whether Pause/Stop from the app work on the real printer (upload, status and
  camera are confirmed), and whether its screen now shows the layer total.
* Prusa: the whole PrusaLink path on a real printer, including the password and the
  storage name on each model.
* USB: a real Marlin or Prusa printer. Some boards need a different speed, and some
  older firmware doesn't support `M155` (the app falls back to asking with `M105` only
  when idle).
* Repetier: a real Repetier printer. Untested so far:
  * whether it answers `ok` after a resend (the app copes either way, through `wait`);
  * whether its screen shows the `M531` name and `M532` progress;
  * whether `G32` suits its probe setup.
  Repetier-Server (network) is not supported.
* MakerBot protocol / Sailfish: a real printer. Still to confirm:
  * that the board answers at 115200 baud after the 2 s start-up wait;
  * the variant byte it actually sends;
  * whether the EEPROM layout of an early Creator Pro 2 matches Sailfish 7.8's
    `EepromMap.hh`.
