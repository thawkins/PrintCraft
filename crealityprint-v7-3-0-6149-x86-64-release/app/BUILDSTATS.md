# Build statistics

Measured on 2026-11-18 on
`TimWorkFed01.local` (Linux, local NVMe filesystem, btrfs), at commit `0f27131`
(PrintCraft 0.0.3).

- `cargo 1.102.0-nightly (ede6cefea 2026-11-11)`
- `rustc 1.102.0-nightly (5df8c02af 2026-11-16)`
- `cargo-nextest 0.9.152`, `cargo-llvm-cov 0.9.1`, `jq`, `python3`

Disk figures are free-space deltas on the filesystem holding the repository,
probed immediately before and immediately after each step. A negative delta is
reported as space *freed*.

## Phase timings

| Step | Command | Time | Disk consumed | Disk free before | Disk free after |
|------|---------|------|---------------|------------------|-----------------|
| clean | `cargo clean` | 1s | freed 3.36 GiB | 739.40 GiB | 742.77 GiB |
| release | `cargo build --release` | 2m 23s | freed 0.11 GiB | 742.77 GiB | 742.88 GiB |
| debug | `cargo build` | 1m 1s | 0.26 GiB | 742.88 GiB | 742.62 GiB |
| tests | `cargo nextest run --no-fail-fast` | 14s | 0.08 GiB | 742.62 GiB | 742.54 GiB |
| coverage | `cargo llvm-cov nextest --no-fail-fast` | 3m 46s | 0.78 GiB | 742.54 GiB | 741.75 GiB |

The free-space deltas understate what each step writes. btrfs releases deleted
space in the background, so space freed by `cargo clean` (which reported 5.5 GiB
removed) was still being returned during the release build, whose row therefore
shows space *freed*. The folders each step leaves, measured with `du` afterwards:

| Folder | Written by | Size |
|--------|------------|------|
| `target/release` | release | 532 MiB |
| `target/debug` | debug and tests | 1211 MiB |
| `target/llvm-cov-target` | coverage | 1029 MiB |

The `tests` step reuses the debug build's dependencies and only compiles the test
binary, so it is much cheaper than `debug`. The `coverage` step builds everything
again with instrumentation in its own folder.

## Line coverage

`overall: 8813/14147 lines = 62.3%`

Method: `cargo llvm-cov nextest --no-fail-fast --json`, with
`--ignore-filename-regex '(^|/)(tests|benches|examples)/|target/'`, so test, bench
and example sources are excluded from both numerator and denominator. PrintCraft
is a single crate, so the figures are summed from `.data[0].files` by source
module (`src/<module>.rs`), with the `printer/`, `x3g/` and `app/` folders listed
file by file.

| Module | Lines covered | Lines total | Coverage |
|--------|---------------|-------------|----------|
| app | 0 | 1130 | 0.00% |
| app/prepare_ui | 0 | 657 | 0.00% |
| app/project_ui | 0 | 257 | 0.00% |
| codec | 59 | 59 | 100.00% |
| device_ui | 595 | 2196 | 27.09% |
| formats | 301 | 326 | 92.33% |
| fsutil | 42 | 56 | 75.00% |
| gcode | 776 | 785 | 98.85% |
| geometry | 82 | 82 | 100.00% |
| gizmo | 195 | 302 | 64.57% |
| heat | 117 | 119 | 98.32% |
| main | 0 | 213 | 0.00% |
| model | 424 | 449 | 94.43% |
| pipeline | 65 | 65 | 100.00% |
| printer/calibration | 127 | 128 | 99.22% |
| printer/camera | 65 | 133 | 48.87% |
| printer/creality | 337 | 452 | 74.56% |
| printer/digest | 85 | 88 | 96.59% |
| printer/discovery | 56 | 114 | 49.12% |
| printer/known | 176 | 183 | 96.17% |
| printer/layer_info | 40 | 43 | 93.02% |
| printer/link | 170 | 187 | 90.91% |
| printer/mod | 238 | 286 | 83.22% |
| printer/moonraker | 150 | 260 | 57.69% |
| printer/prusalink | 247 | 350 | 70.57% |
| printer/sailfish | 436 | 566 | 77.03% |
| printer/serial_log | 123 | 126 | 97.62% |
| printer/usb | 546 | 639 | 85.45% |
| profiles | 190 | 190 | 100.00% |
| project | 441 | 454 | 97.14% |
| recent | 113 | 117 | 96.58% |
| slicer | 1128 | 1187 | 95.03% |
| theme | 0 | 47 | 0.00% |
| thumbnail | 168 | 172 | 97.67% |
| tool_icons | 0 | 58 | 0.00% |
| view3d | 105 | 275 | 38.18% |
| widgets | 0 | 36 | 0.00% |
| x3g/command | 207 | 209 | 99.04% |
| x3g/machine | 74 | 80 | 92.50% |
| x3g/mod | 119 | 186 | 63.98% |
| x3g/query | 297 | 308 | 96.43% |
| x3g/translate | 519 | 577 | 89.95% |

The window code is low by construction:
- `app`, `app/*`, `device_ui`, `view3d`, `theme`, `widgets` and `tool_icons` draw
  the egui window, and no test opens one. The device page's logic that doesn't
  draw (connecting, locating a printer, "print again") is tested.
- `main` is the command line. It is run by hand, not by the test binary.
- In `printer/`, the parts that need a real network (discovery's scan, the camera
  stream, Moonraker's and PrusaLink's live calls) are covered only by the
  pretend-printer tests that exist. `x3g/mod`'s `export_machine` reads the saved
  EEPROM copy, which tests deliberately skip.

## Test totals

`cargo nextest run --no-fail-fast` (the green run):

```
     Summary [   2.258s] 198 tests run: 198 passed, 0 skipped
```

The coverage run (`cargo llvm-cov nextest`) ran the same suite with the same
result:

```
     Summary [   2.221s] 198 tests run: 198 passed, 0 skipped
```

Summed over **the one test binary**: **198 tests passed, 0 failed, 0 skipped**.

The suite runs under nextest, one process per test, as the project's AGENTS.md
asks (`.config/nextest.toml`). Some tests change process-wide state such as the
`TZ` variable, so plain `cargo test`, which runs them as threads of one process,
is not the supported runner. There are no doctests, because PrintCraft is a
binary crate.

## Defects fixed during measurement

None. Every step passed on its first run.
