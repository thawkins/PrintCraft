# FlashForge and MakerBot printers: implementation plan

Status, 2026-11-18:
* **Phase 0** (checking the facts) is done.
* **Step 3a** (the read-only Sailfish connection) is written and tested against a
  pretend printer. It has **not yet been run on the real printer**.
* **Steps 3b and 3c** (the translator and X3G export) are written and checked by tests,
  an independent decode of an exported file, and a comparison with GPX 2.6.8.
* **Steps 3d, 3e and 3f** and the core of **track 1** are written and tested against a
  pretend printer:
  * printing over USB, with pause, stop and the controls;
  * the SD card's files;
  * recording onto the card where the firmware can;
  * a configurable nozzle count, a nozzle for each part, the support nozzle, the second
    filament, and Ditto.

  **Nothing has been printed on the real printer yet.**
PrintCraft version at the time: 0.0.3.

## What phase 0 found

The test printer is a **FlashForge Creator Pro 2**, an **early unit that runs Sailfish
firmware**, with **both nozzles on one carriage** and **Ditto printing** in its menu
(all confirmed by the owner). FlashForge's
current user guide describes later units, which run FlashForge's own firmware. So the
Creator Pro 2 has existed in two forms, and PrintCraft should support both:

| | Sailfish printers: early Creator Pro 2 **(the test printer)**, Creator Pro, Creator X, Replicator 1/2/2X | Later Creator Pro 2 (FlashForge firmware) |
|---|---|---|
| Firmware | Sailfish 7.x (MakerBot MightyBoard family) | FlashForge's own (the guide shows V1.2 20200420; upgrades `creator-pro2_v1.3.20200817`) |
| Print files | `.x3g` (binary S3G commands) | `.g` / `.gx` (the guide: "Output: GX/G") |
| USB | Serial port (shows up as `/dev/ttyACM*`), binary S3G packets | USB **bulk** device (vendor 0x2B71, product 0x000E), ASCII G-code prefixed with `~` |
| Nozzles | One, or two on one carriage (the test printer: two, with Ditto) | Two independent carriages (IDEX), with duplicate and mirror modes |

The work is three tracks:

1. **Dual-nozzle printing in PrintCraft** (general, configurable per printer; the
   Creator Pro 2 needs it).
2. **MakerBot / Sailfish printers**: X3G export, the S3G USB protocol, and upload to
   the SD card. **This is what makes the test printer work**, and it is first in the
   order of work (track 3 below).
3. **FlashForge-firmware printers**: the FlashForge dialect, `.g`/`.gx` export and the
   FlashForge USB connection. Needed for later Creator Pro 2 units and other FlashForge
   models. No such printer is available to test with (track 2 below).

Sources read in phase 0:

| Source | What it gave |
|---|---|
| FlashForge *Creator Pro 2 User Guide* (FlashForge's PDF, read as text) | Specs, firmware versions, file formats, the rule that the SD card must be in for USB printing, IDEX mode volumes |
| MakerBot `s3g` repository, `doc/s3gProtocol.md` | Packet format, reply codes, all S3G command layouts |
| Sailfish firmware source (`jetty840/Sailfish-MightyBoardFirmware`, version 7.8): `Packet.hh`, `Packet.cc`, `Commands.hh`, `platforms.py` | Payload limit, CRC, Sailfish-only command numbers, the Creator X/Pro build settings |
| OctoPrint-FlashForge plugin (`Mrnt/OctoPrint-FlashForge`), its source and wiki "G Code Reference" | The FlashForge USB transport, the `~` command framing, the upload sequence, the dialect, and the Creator Pro 2's USB ids. **GPL-3: only facts are used, no code.** |
| `ztripez/flashforgefinder-protocol` | The same `~M601` text protocol over a network socket (Finder) |
| GPX (`markwal/GPX`) README | Machine ids (`fcp` = FlashForge Creator Pro, `r2`, `r2x`). Its machine tables weren't read yet (see track 3). |

The clean-room rule in `AGENTS.md` covers the Creator application only. Nothing
here comes from FlashPrint's program files. Anything taken from a third-party
reverse-engineering project is marked *(community)* and needs to be checked on the
printer.

---

## Track 1: dual-nozzle printing

### What the test printer needs (early Creator Pro 2, Sailfish)

* Two extruders on **one carriage**, a fixed X offset apart (Replicator 1 Dual style),
  0.4 mm nozzles, 1.75 mm filament.
* **Ditto printing**: both nozzles print the same part at once, side by side.
* Tool changes and the nozzle offset follow Sailfish (track 3).

### What later, FlashForge-firmware Creator Pro 2 units need (from the guide)

* Two independent extruders: **T0 = right, T1 = left** *(community)*. Both have 0.4 mm
  nozzles and use 1.75 mm filament.
* Build volume 200 × 148 × 150 mm; in **mirror** mode 80 × 148 × 150; in **duplicate**
  mode 95 × 148 × 150.
* Material pairs the guide names: PVA + PLA, HIPS + ABS, PLA + PLA, ABS + ABS (soluble
  supports are the main use).
* The guide has calibration screens for the X and Y offset between the two nozzles,
  and an expert mode for adjusting it. The printer stores the offset itself, so the
  slicer does not apply it *(to confirm on the printer)*.

### What PrintCraft has today

A printer profile has one nozzle (`profiles::Printer`), one filament, and `gcode.rs`
writes a single tool. Objects have no extruder setting.

### Extruders are a printer setting, not an assumption

Most printers have **one extruder**, and that stays the default. Nothing about two
nozzles may appear, or change the output, unless the printer profile says it has more:

* The number of extruders is set in the printer profile: **1 or 2**, kept as a list so
  more can be allowed later. Presets set it to match the machine: the Creator Pro /
  early Creator Pro 2 has 2, every current preset has 1. A custom printer can be
  changed in the profile editor.
* The printer kind is a separate setting:
  * **shared carriage** (two nozzles on one head, like the test printer), which offers
    **Ditto**;
  * **independent (IDEX)**, like later FlashForge-firmware Creator Pro 2 units, which
    offers duplicate and mirror.
* With one extruder:
  * the window shows no extruder choices, no prime tower and no print-mode setting;
  * the G-code is **byte for byte the same as today**. A test slices the existing test
    plates with a one-extruder profile and compares the output with the current
    output.
* Saved profiles and projects from before this change have no extruder setting. They
  load with one extruder. Projects that name extruder 2 on a printer that has only one
  fall back to extruder 1, with a note in the status line.

### What is done (2026-11-18)

* **Profiles:** `Printer::extruders` (1 or 2, "Nozzles" in the printer editor). The
  Creator Pro (Sailfish) preset has 2; every other preset has 1, and old projects load
  with 1.
* **Parts and settings:** each part has a nozzle ("Nozzle" under Selected object). The
  "Two nozzles" group sets the support nozzle and Ditto. The "02" filament chip sets the
  second nozzle's filament (or "Same as 01"). All of these are saved in the project.
* **Slicing:** the plate is sliced once, as before, so supports and the skirt stay shared.
  Each finished path is then given a nozzle:
  * supports go to the support nozzle;
  * skirt, brim and raft go to the first nozzle any part uses;
  * everything else goes to the part whose footprint it lies in.

  Each layer prints the selected nozzle's paths first, then the other's. That is one tool
  change per layer, alternating, and each change retracts, switches, resets E and
  primes.
* **G-code:** the second nozzle is heated and waited for at the start, purged with its
  own two lines, and switched off at the end.
* **One nozzle:** the G-code is unchanged, and a test checks it is identical.

**Added after (2026-11-18, the owner's choices):**
* **Primary nozzle: a printer setting** ("Primary nozzle" in the printer editor, right
  (T0) by default, which matches the firmware's tool 0, saved in projects as
  `primary_nozzle`).
  * New parts, supports, the skirt and one-nozzle prints use it, and it has the main
    filament ("01"); "02" is the other nozzle's.
  * A plate printed by one nozzle heats only that nozzle.
  * Changing the primary moves the parts and supports that were on the old one. Those
    set to the other nozzle stay. A part saved without a nozzle loads onto the printer's
    primary.
* **Anti-ooze at tool changes.** The nozzle set aside is pulled back by the
  "Tool-change retraction" (8 mm, estimated) and left at "Standby drop" (15 °C, estimated)
  below its printing temperature. Before it prints again it is heated back (`M109 T`, a
  short wait) and primed by exactly what it was pulled back. The second nozzle is set
  aside the same way after its start-up purge.
* **Retraction for every printer:** "Retract on travels over" (the old fixed 2 mm),
  "Retract on layer change" and "Wipe". The defaults leave the G-code byte-for-byte as
  before, which was checked against saved output for three printers.

* **Prime tower** ("Prime tower" under Two nozzles, on by default). It is a 12 mm square
  10 mm in from the back-right corner.
  * After each tool change the incoming nozzle primes on it before printing the part. A
    layer without a change prints it first.
  * It is built up to the last layer with a tool change, never higher.
  * Slicing is refused while a part is within 3 mm of it.
  * One-nozzle plates and Ditto get no tower.
* **Per-nozzle estimates:** filament, weight and cost are worked out for each nozzle with
  its own filament. They show in the estimate card and in the G-code footer.
* **Preview:** "Colour by nozzle" (T0 blue, T1 orange) next to the path types. With
  Ditto, the copy is drawn dimmed one nozzle spacing to the side (left when T0 prints,
  right when T1 does).
* **Ditto checks.** The nozzle spacing is a printer setting ("Nozzle spacing", 33 mm by
  default: GPX's `r1d` offset).
  * Before slicing, the parts must fit within the spacing less 3 mm, and the copy must
    land on the plate.
  * After slicing, the same check runs on everything the job prints, skirt, brim and raft
    included, because those widen the job and would collide with the copy's. A job that
    fails shows why and can't be sent or exported.

* **Tower position** under Two nozzles: automatic (back right), or set in mm from the
  plate's front-left corner. Saved in the project. Slicing is refused if the tower would
  stand partly off the plate or within 3 mm of a part.

* **Ooze shield** ("Ooze shield" under Two nozzles, off by default, 3 mm away by
  default).
  * It is one closed line around the convex hull of everything the print lays down
    (parts, supports, skirt, brim, raft; not the tower), so it can't cross any of them on
    the first layer.
  * It is the same shape on every layer and built up to the last tool change. Each layer
    prints it first, with the selected nozzle.
  * After slicing, a shield off the plate or a tower inside it is refused, and the job
    can't be sent or exported.

**Not done yet:**
* independent carriages (IDEX) and Klipper's tool changes.

### Plan

| Step | Work |
|---|---|
| 1a. Profiles | `Printer` gains `extruders: Vec<Extruder>` (nozzle diameter, name such as "Right"/"Left", retraction for each tool change; 1 entry by default) and `carriage: Shared \| Independent`. Both appear in the printer profile editor. The project keeps one filament for each extruder. |
| 1b. Assigning | Each object gets an extruder (like a Cura "per-model extruder" setting). Supports get their own extruder choice ("Support: left (PVA)"). The plate view colours each object by its filament. |
| 1c. Slicing | Each layer's paths are grouped by extruder. The order alternates (the last tool of one layer starts the next) to halve the tool changes. |
| 1d. Tool changes | A tool change routine for each firmware family: retract, park/standby temperature for the idle nozzle, select the tool (`T1` for Marlin; `M108 T1` for FlashForge), wait for temperature, prime. A **prime tower** (optional, on by default for two materials) wipes the new nozzle before it prints. An ooze shield is later work. |
| 1e. Print modes | A print mode setting, shown only for two-extruder printers. Shared carriage: "Normal / Ditto"; Ditto limits the plate to the bed width minus the nozzle offset and shows the copy in the preview. IDEX: "Normal / Duplicate / Mirror", restricted to the mode's volume, with the firmware's mode command. In every copy mode both nozzles use the same filament settings. |
| 1f. Preview and estimates | The preview colours by extruder. Filament use and cost are worked out for each extruder. |

Tests: a one-extruder profile gives the same G-code as before the change and shows no
dual-nozzle controls. An old profile loads as one extruder. A two-object plate gives
exactly one tool change for each layer where both
appear, with the routine in the right order. The prime tower appears only on layers
with a change. Supports on the support extruder go to the support tool. Duplicate mode
refuses a plate wider than the mode's volume.

---

## Track 2: FlashForge printers (the Creator Pro 2)

### The G-code dialect *(community: OctoPrint-FlashForge wiki; check each on the printer)*

FlashForge firmware follows Marlin for moves, but several commands differ. These matter
most:

| Purpose | Marlin (what `gcode.rs`/`heat.rs` write now) | FlashForge |
|---|---|---|
| Wait for the nozzle | `M109 S210` | `M104 S210 T0`, then **`M6 T0`** |
| Wait for the bed | `M190 S60` | `M140 S60`, then **`M7`** |
| Select a tool | `T1` | **`M108 T1`** (plain `T1` is reported not to be understood) |
| Duplicate / mirror mode | — | **`M109 T2`** duplicate, **`M109 T1`** mirror. **`M109` means something else here:** sending the Marlin wait command would switch the IDEX mode. |
| Move to the endstops | `G28` | `G28` works; `G161`/`G162` home to min/max |
| Load the stored nozzle offsets | — | `M132` (meaning not certain) |

So a FlashForge printer profile needs its own **start/end and heat routines**, just as
Klipper has one today (`profiles::Printer::klipper`, `heat.rs`). That one flag
becomes a `firmware: FirmwareFlavor` enum (`Marlin`, `Klipper`, `FlashForge`). Saved
profiles that have `klipper: true` load as `Klipper`. A test slices with the
FlashForge flavor and checks that the output contains no `M109 S`/`M190`/bare `T` line.

### Print files

* **`.g`**: the guide's "How to create .g file" saves it to the SD card for printing.
  Whether a `.g` file is plain G-code or has a header **is not stated** anywhere read.
* **`.gx`**: FlashPrint's format. Third-party notes describe it as a header with a
  preview picture, followed by the G-code. No public description of the header was
  found in phase 0.
* **Step 0 of this track:** export one small model from FlashPrint in both formats and
  put them in `reference/`. That folder is where `AGENTS.md` says reference
  material belongs. With real files in hand the header can be documented from the
  bytes, and the export can be checked byte for byte against what the printer accepts.
  Until then, export writes **`.g` as plain FlashForge-dialect G-code**. The
  OctoPrint-FlashForge plugin uploads slicer G-code to these printers this way.

### The USB connection *(community, then confirmed on the printer)*

* The printer is not a serial port. It shows up as a USB device with vendor id
  **0x2B71** (FlashForge) and product id **0x000E** (Creator Pro 2), and it talks over
  **bulk endpoints**: the first in/out pair carries commands, a second pair (if present)
  carries file data. So the `serialport` crate can't reach it. This needs a USB library
  (`rusb`, the Rust binding to libusb). On Linux the user needs a udev rule to open the
  device without root. The app shows the exact rule when access is refused.
* **Framing:** every command is sent as `~` + command + `\r\n` (`~M105\r\n`). The printer
  answers `CMD M105 Received.`, then data lines, then `ok`. Read in 512-byte chunks until
  the reply ends in `ok`.
* **Session:**
  * `~M601 S1` takes control and `~M602` releases it.
  * `~M115` gives the machine information, including the firmware version for the
    badge.
  * Some printers drop the connection after about 4 s of silence, so the worker sends
    `~M119` (machine status) as a keep-alive and reads `MachineStatus:`/`MoveMode:` from
    the reply.
* **Only some commands are answered during a print** ("unbuffered": status,
  temperature, position, progress). Others are queued and answered later, so while
  printing the worker sends only status queries. Jogging and homing are refused with a
  message.
* **Printing (upload, then print):**
  1. Check that the printer is ready (`M119`), and that no print is running.
  2. `~M28 <size in bytes> 0:/user/<name>`. The reply must not say "open failed".
  3. Send the file bytes raw (no `~`, no line numbers) on the file endpoint, in fixed
     chunks. The upload shows its progress.
  4. `~M29` to close the file.
  5. `~M23 0:/user/<name>` selects the file **and starts the print** (on FlashForge,
     selecting is starting).
  6. Then: progress from `~M27` (`SD printing byte N/M`), temperatures from `~M105`
     (`T0:… T1:… B:…`), pause/resume `~M25`/`~M24`, cancel `~M26`, emergency stop
     `~M112`, the enclosure light `~M146 r g b F0`.
* **The SD card must be in the printer** for any USB print (the guide says so
  explicitly). The worker says so if `M28` fails.
* There is no command to list the card's files on these printers *(community)*. The
  connection's `files` ability stays off.
* **Connection kind:** new `Api::FlashForgeUsb` (key `"flashforge-usb"`), discovered by
  listing USB devices with vendor 0x2B71 next to the serial ports in the USB panel.
  The worker lives in `src/printer/flashforge.rs` and keeps the existing
  `Worker`/`LinkState` contract. The serial log records the `~` traffic (and the upload
  as one line with its size, not the bytes).
* **The network version:** FlashForge printers with Wi-Fi/Ethernet take the same `~M601`
  text over a TCP socket. The Creator Pro 2 has no network ("Connectivity: SD card,
  USB cable"), so this is noted for later only.

### Steps

| Step | Work | Done when |
|---|---|---|
| 2a | Reference files from FlashPrint in `reference/` (done by the owner). Run `lsusb -v -d 2b71:` with the printer plugged in and save it in `reference/`. | Endpoints and file formats are known, not inferred. |
| 2b | `FirmwareFlavor` + the FlashForge start/end/heat/tool routines + the Creator Pro 2 preset (200 × 148 × 150, two extruders). | Sliced output passes the dialect test. Export writes `.g`. |
| 2c | `rusb` dependency + `printer/flashforge.rs`: connect, M601, M115 badge, temperatures, keep-alive, controls when idle. `cargo audit` must stay clean with the new dependency. | Connected to the real printer: temperatures shown, a jog and a home work, the log shows the conversation. |
| 2d | Upload and print, progress, pause, resume, cancel. | A test cube prints from PrintCraft over USB. Cancel leaves the heaters off. |
| 2e | `.gx` export with a header, if 2a shows it is needed or better (picture on the printer's screen). | The printer lists and prints the `.gx` with its picture. |
| 2f | Dual-nozzle prints and duplicate/mirror (after track 1). | A PLA + PVA print and a duplicate print succeed. |

Tests without the printer: the dialect test, framing and reply parsing (`CMD … Received.`
… `ok`), `M27`/`M105`/`M119` parsing from recorded replies, and the upload sequence
against a fake transport (a `trait Transport` so the worker can be tested without USB).

---

## Track 3: MakerBot / Sailfish (X3G export and S3G over USB)

For the **test printer (early Creator Pro 2 on Sailfish)**, and also the original
Creator Pro, Creator X, Replicator 1/2/2X and their clones. Facts checked in phase 0:

* **Packets:** `0xD5`, payload length, payload (**at most 32 bytes**: Sailfish
  `MAX_PACKET_PAYLOAD`), CRC. The CRC is the iButton/Maxim CRC-8 (Sailfish uses
  avr-libc's `_crc_ibutton_update`: polynomial 0x8C reflected, start 0) over the
  payload only. Integers are little-endian. One packet is in flight at a time.
* **Replies:**
  * `0x81` ok;
  * `0x80` generic error and `0x83` CRC mismatch: resend, up to 5 times;
  * `0x82` buffer full: resend later, without limit;
  * `0x84` query too big;
  * `0x85` unsupported command;
  * `0x87` downstream timeout;
  * `0x88` tool lock timeout;
  * `0x89` build cancelled;
  * `0x8A` printing from SD;
  * `0x8B` overheat shutdown;
  * `0x8C` packet timeout.
  * SD commands add their own status byte (0 = ok, 1 = no card, …).
* **Host queries:**
  * 0 version, and 27 advanced version (gives the variant byte);
  * 1 init;
  * **2 available buffer size (uint32) and 3 clear buffer** (the first draft had these
    two swapped);
  * 7 abort, 8 pause/resume toggle, 10 tool query, 11 is finished;
  * 14 capture to file (8.3 name, null-terminated), 15 end capture (returns the byte
    count), 16 play back a file;
  * 18 next SD file name, 20 build name, 21 extended position (5 × int32 steps +
    endstop bits), 22 extended stop, 23 motherboard status bits, 24 build statistics.
* **Tool query subcommands:** 2 nozzle temperature, 22 tool ready, 30 bed temperature,
  32/33 nozzle/bed target, 35 bed ready, 36 tool status bits.
* **Tool action subcommands:** 3 set nozzle temperature, 12 fan on/off, 13 extra output,
  31 set bed temperature.
* **Action commands (and the content of `.x3g`):**
  * 131/132 home to min/max (axes, step interval µs, timeout s);
  * 133 delay ms, 134 change tool, 135/141 wait for tool/bed (tool, interval ms,
    timeout s), 136 tool action, 137 enable/disable axes, 140 set position;
  * 149 message, 150 build percent, 153/154 build start/end;
  * **155 X3G move:** 5 × int32 steps, uint32 DDA rate in steps/s, uint8 relative-axes
    mask, float32 distance in mm, uint16 feed rate in mm/s × 64;
  * Sailfish adds 156 acceleration on/off, **157 stream version**, and 158 pause at a
    Z height (numbers from `Commands.hh`).
* **Variant byte from 27:** the spec says 0x80 = Sailfish. The firmware's own enum says
  `SAILFISH = 2`. The value actually sent was **not settled**, so detection must accept
  either.
* **Upload to the SD card** over S3G: command 14 opens a file, every following command is
  written to it instead of run, then 15 closes it. Upload is therefore the same packet
  stream as printing, just captured.
* **Creator X/Pro build settings** (`platforms.py`, `ff_creatorx-2560`): ATmega2560,
  nozzle offset X 32.01 mm (3201), auto-level, RGB LED.
* **FlashForge Creator Pro machine values** from GPX's `std_machines.h`. GPX treats
  `fcp` (FlashForge Creator Pro) as another name for its `r1d` machine, the
  Replicator 1 Dual. Facts only; no GPX code is used.

  | Axis | Max feed rate | Home feed rate | Length | Steps/mm | Endstop |
  |---|---|---|---|---|---|
  | X | 18000 mm/min | 2500 mm/min | 227 mm | 94.117647 | max |
  | Y | 18000 mm/min | 2500 mm/min | 148 mm | 94.117647 | max |
  | Z | 1170 mm/min | 1100 mm/min | 150 mm | 400 | min |
  | A, B (extruders) | 1600 mm/min | — | — | 96.2752 | — |

  * Two extruders and a heated bed.
  * Nozzle offset 33 mm in X. Sailfish's own Creator X/Pro build uses 32.01 mm.
  * Nozzle 0.4 mm, filament 1.75 mm.
* **Early Creator Pro machines are clones of the MakerBot Replicator 1 Dual** (the
  owner's information). This matches GPX treating `fcp` as `r1d`. Both nozzles sit on
  **one carriage** at a fixed X offset. They share the same electronics family
  (MightyBoard), Sailfish and the S3G protocol. So the `r1d` values above are the
  starting definition for the early Creator Pro family, and the plan's shared-carriage
  case (`carriage: Shared`, track 1) is the one these machines need.
* **The test printer's own values are not known yet.** The `r1d` values are a strong
  starting point, but a clone may differ (axis lengths, offsets, motors). Before any
  movement code runs on it, the values come from the
  printer itself: its EEPROM (read with query 12, the offsets in Sailfish's
  `EepromMap`), the firmware version (queries 0 and 27) and its info screen. The
  `fcp` values serve only as a check.
* **Two nozzles on one carriage** (Replicator Dual style; the test printer, confirmed)
  is what Sailfish supports as standard:
  * `134` selects the tool;
  * the X offset between the nozzles is stored in EEPROM, and the translator applies
    it to the second nozzle's moves;
  * **Ditto printing** (confirmed in the test printer's menu): both nozzles extrude at
    once, so the second nozzle prints a copy of the part beside the first, one nozzle
    offset away.

  For PrintCraft this means two things:
  * The plate is limited to the width a Ditto copy leaves free: the bed width minus
    the nozzle offset.
  * The preview shows the copy, so the person sees where the second part lands.

  **Ditto is a printer setting, not a job command.** Sailfish keeps it in EEPROM
  (`DITTO_PRINT_ENABLED`, offset 0x0FFF in `EepromMap.hh`), set from the printer's menu.
  Step 3a reads it, so PrintCraft can know whether Ditto is on:
  * when it is, the plate is limited and the preview shows the copy;
  * the job itself carries nothing to switch it.

  Switching it from the app would mean writing the EEPROM (command 13). That is left
  out unless the owner asks for it.
* Independent carriages (IDEX) under Sailfish are not needed for the test printer and
  are left out of this plan.
* **USB:** a Sailfish board appears as a serial port, so it uses the port list and baud
  setting PrintCraft already has. Sailfish's usual speed is 115 200 baud *(to confirm)*.

The design from the first draft still holds for this track:
* `src/x3g/` (encoding, machines, a translator from the app's G-code to S3G);
* `printer/sailfish.rs` (the USB link);
* a protocol probe after a text timeout;
* a GPX oracle test.

Steps:

| Step | Work | Done when |
|---|---|---|
| 3a | **Written, not yet run on the printer** (`src/x3g/`, `src/printer/sailfish.rs`). Read-only connection: version (0, 27), temperatures (10), motherboard status (23), build statistics (24), position (21), the whole EEPROM (12), saved as `sailfish-eeprom.bin` and summarised in the log. Badge "MakerBot protocol · Sailfish x.y". Never sends 3 or 7 (Sailfish restarts the board on both). | The test printer's versions, temperatures and EEPROM values are in the log, and the machine values are written into this file. |
| 3b | **Written, not yet printed** (`src/x3g/command.rs`, `machine.rs`, `translate.rs`). S3G action commands; the Creator Pro machine definition (GPX `r1d` values, replaced by the printer's EEPROM copy when one has been saved); the G-code → S3G translator for single-nozzle prints. X/Y/Z are absolute steps from the bed's centre. The extruder is relative and negative (as GPX writes it), with rounding carried over. Speeds are capped at each axis's limit. G29/G32 are dropped with a note, and fan speeds become on/off. An unknown command is an error naming its line. **Compared with GPX 2.6.8** (`gpx -r -m fcp`, see below). | Unit tests, and the GPX oracle where GPX supports the machine. |
| 3c | **Written, not yet printed.** The "FlashForge Creator Pro (Sailfish)" preset (227 × 148 × 150) and a "MakerBot / Sailfish" printer setting. Export saves `.x3g` for such a printer, and `printcraft --slice … --output OUT.x3g --printer 11` does the same. An exported test model decodes cleanly with an independent decoder. Its extrusion total matches the slicer's filament estimate (2501.6 mm vs 2.50 m). The purge line sits at X −112 mm, 1.5 mm inside the edge GPX gives: to check on the printer. | A cube exported as X3G prints from the SD card on the test printer. |
| 3d | **Written, not yet printed.** Streaming over USB, paced by "buffer full": print, progress, pause/resume (8), stop (22, then heaters off), jog (relative 155), home, heaters, part fan. Jog and home are refused while printing; there is no speed change. | The cube prints over USB. Stop leaves both heaters off. |
| 3e | **Written, not yet printed.** The SD file list (18) and printing a file from the card (16) work on any Sailfish. Recording a job onto the card (14, the job, 15, then 16) is tried first, but **stock Sailfish is built without it** (`S3G_CAPTURE_2_SD` is off in every board configuration), so on most printers the app falls back to streaming and says why. To print from the card without the computer, export the X3G file and copy it onto the card. | A job uploaded from PrintCraft prints from the card. |
| 3f | **Written, not yet printed.** `T0`/`T1` become tool changes (134). The firmware applies the nozzle offset from its EEPROM. `M104/M109 T1` heat the second nozzle, and its extrusion goes to the B axis. Ditto heats both nozzles and limits the plate to 30 mm across (33 mm nozzle spacing less a gap). Ditto itself is switched on in the printer's menu (EEPROM). The preview copy is not done. | A two-colour or PLA + PVA print succeeds with the colours lined up, and a Ditto print gives two matching parts. |

---

### Comparison with GPX 2.6.8 (2026-11-18)

The XYZ test model was sliced with preset 11, and the same G-code (shifted to the bed's
centre) was converted with `gpx -I -r -m fcp`. Both X3G files were decoded and compared
command by command. GPX's framed output (`-F`) and its USB conversation with a pretend
Sailfish (`gpx -s` on a pseudo serial port) were checked as well.

* **Same:**
  * framing and CRC: all 55,660 of GPX's packets pass PrintCraft's CRC-8, and none is
    over 32 bytes;
  * the layout of every command both write;
  * 55,532 moves have the same X/Y/Z/A steps (within one step);
  * extrusion direction and retraction;
  * move distances and feed rates;
  * the temperature questions sent over USB (tool 0 and 1 temperature and target, then
    the bed through tool 0) and the build-statistics query.

  Unit tests now hold GPX's exact packets as reference bytes
  (`matches_packets_written_by_gpx`).
* **Fixed after the comparison:**
  * The part fan (M106/M107) used tool subcommand 12. That is the nozzle's *heatsink*
    fan, so every print switched it off at the start. It now uses 13, the board's
    "extra" output that drives the part fan, as GPX does.
  * Heat waits gave up after 600 s, after which the printer would carry on cold. They now
    have no time limit (65535), as GPX writes them.
  * Homing X and Y together now runs at GPX's speed (361 µs per step, 1/√2 of the homing
    feed rate on each axis) instead of 255 µs.
  * A move much shorter than one step that still crosses a step boundary could ask for a
    step rate above the axis's limit. The step rate is now capped at the moving axis's
    maximum.
* **Different on purpose:**
  * Speed: PrintCraft works out the step rate from the steps actually taken (the
    difference of the rounded positions). GPX rounds the move's length instead, so its
    rate is up to a few percent off on short moves (38 vs 37 steps over 0.4 mm). This
    affects 8,620 short moves.
  * Moves shorter than a step: GPX drops some that PrintCraft keeps (243 vs 108), each
    under 0.02 mm, because the two round in different places. Neither loses track of the
    position.
  * After homing, PrintCraft recalls the printer's stored home position (144,
    ReplicatorG's `M132`). Converting RepRap-style G-code, GPX instead sets the
    position to 0, 0, 0 at the first `G92`, which would make the home point the bed's
    centre.
  * PrintCraft writes the stream version, build start and screen message at the start
    (optional in the specification). GPX leaves them out when converting this way.

## Order of work

1. **3a**: read-only Sailfish connection, so the test printer's real values are known.
2. **3b → 3c**: translator and X3G export (single nozzle). This is useful at once:
   files on the SD card print.
3. **3d → 3e**: USB streaming, then upload to the SD card.
4. **Track 1**: configurable extruders, then **3f** (two nozzles and Ditto on the test
   printer).
5. **Track 2** (FlashForge firmware): when a tester with such a printer is available.
   The dialect and `.g` export (2b) can be done without one.

Each step is one PR with tests. CHANGELOG, FEATURES.md and PRINTER_LINK.md are updated
in the same PR. Anything confirmed on the printer moves from *(community)* to confirmed
in this file.

## Open questions for the owner

1. What does the test printer's info screen show for the firmware (Sailfish version
   and build name)? With it plugged in, which port appears (`ls /dev/ttyACM*`), and
   what does `lsusb` list for it?
2. An `.x3g` from FlashPrint (or another slicer) that prints well on it, placed in
   `reference/`, would be a second check on the translator besides GPX.
3. For track 2 later: recording FlashPrint's USB traffic would settle the FlashForge
   upload details. That means observing a closed program, so it's your call whether
   it's acceptable under the project's clean-room rule.
