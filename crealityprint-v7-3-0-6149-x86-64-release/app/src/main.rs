//! PrintCraft: a simpler desktop slicer for 3D printers.
//!
//! Usage:
//!   printcraft [MODEL.stl ...]                      open the app (optionally with models)
//!   printcraft --screenshot OUT.png [MODEL.stl ...] open, draw ~120 frames, save a picture, quit
//!   printcraft --slice MODEL.stl [...] --output OUT.gcode [--quality fine|standard|draft]
//!              [--filament pla|petg|abs|tpu] [--printer 1-11] [--supports] [--brim]
//!                                                   slice without a window
//!
//! `printcraft --help` lists every option (preview, printer connection and more).

mod app;
mod codec;
mod cut;
mod device_ui;
mod formats;
mod fsutil;
mod gcode;
mod gcode_template;
mod geometry;
mod gizmo;
mod gyroid;
mod heat;
mod history;
mod honeycomb;
mod layer_events;
mod model;
mod modifier;
mod motion;
mod overrides;
mod pipeline;
mod printer;
mod profiles;
mod project;
mod recent;
mod repair;
mod settings_registry;
mod slicer;
mod split;
mod theme;
mod thumbnail;
mod tool_icons;
mod travel;
mod view3d;
mod widgets;
mod x3g;

use eframe::egui;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
PrintCraft - a simple slicer for 3D printers

Usage:
  printcraft [MODEL.stl ...]
  printcraft --preview MODEL.stl [...]      open the models and slice them straight away
  printcraft --connect ADDRESS                open the Device page and connect to a printer
                                              (a serial port such as /dev/ttyUSB0 or COM3
                                              connects over USB at 115200 baud)
  printcraft --device                         open on the Device page (known printers)
  printcraft --tool move|scale|rotate MODEL   start with this selection tool
  printcraft --file-menu                      open with the File menu showing
  printcraft --connect ADDRESS --camera big|max
                                              show the printer's camera enlarged or maximised
  printcraft --screenshot OUT.png [MODEL.stl ...]
  printcraft --slice MODEL.stl [MODEL.stl ...] --output OUT.gcode [options]
  printcraft --slice MODEL.stl [...] --output OUT.x3g --printer 11 [options]
                                              an X3G file for a MakerBot / Sailfish printer

Slicing options:
  --quality fine|standard|draft   (default standard)
  --filament pla|petg|abs|tpu     (default pla)
  --printer 1-11                  printer preset (default 1, 220 x 220 mm):
                                  1-4 generic, 5 Prusa MK4S, 6 Prusa Core One,
                                  7 Prusa MK3S+, 8 Prusa MINI+, 9 Prusa XL,
                                  10 Creality Ender-3 V3 KE,
                                  11 FlashForge Creator Pro (Sailfish, X3G)
  --supports                      add supports under overhangs (where they reach the
                                  plate they stand on a small raft)
  --no-support-raft               no raft under supports
  --brim                          print a brim instead of a skirt
  --raft                          print the model on a raft
  --infill-pattern lines|grid|triangles|gyroid|honeycomb|cubic|adaptive-cubic|
                                  lightning
  --seam aligned|nearest|rear|random
  --ironing                       smooth top surfaces with an ironing pass
  --vase                          spiral vase mode
  --fuzzy                         fuzzy skin on outer walls
  --fuzzy-top                     textured top surface (random lift up to 0.1 mm)

Models can be STL, OBJ or 3MF files.
";

#[derive(Default)]
struct Args {
    models: Vec<PathBuf>,
    screenshot: Option<PathBuf>,
    slice: bool,
    preview: bool,
    connect: Option<String>,
    device: bool,
    tool: Option<String>,
    file_menu: bool,
    camera: Option<bool>,
    output: Option<PathBuf>,
    quality: Option<String>,
    filament: Option<String>,
    printer: Option<usize>,
    supports: bool,
    brim: bool,
    raft: bool,
    no_support_raft: bool,
    ironing: bool,
    vase: bool,
    fuzzy: bool,
    fuzzy_top: bool,
    infill_pattern: Option<String>,
    seam: Option<String>,
}

fn parse_args(raw: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args::default();
    let mut it = raw.peekable();
    while let Some(arg) = it.next() {
        // The next word, unless it is another option ("--output --brim" is a mistake).
        let mut value = |name: &str| {
            it.next_if(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("printcraft {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--screenshot" => a.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--slice" => a.slice = true,
            "--preview" => a.preview = true,
            "--connect" => a.connect = Some(value("--connect")?),
            "--device" => a.device = true,
            "--tool" => a.tool = Some(value("--tool")?),
            "--file-menu" => a.file_menu = true,
            "--camera" => {
                let v = value("--camera")?;
                a.camera = Some(match v.as_str() {
                    "big" => false,
                    "max" => true,
                    _ => return Err(format!("--camera expects big or max, got \"{v}\"")),
                });
            }
            "-o" | "--output" => a.output = Some(PathBuf::from(value("--output")?)),
            "--quality" => a.quality = Some(value("--quality")?),
            "--filament" => a.filament = Some(value("--filament")?),
            "--printer" => {
                let v = value("--printer")?;
                a.printer = Some(
                    v.parse()
                        .map_err(|_| format!("--printer expects a number, got \"{v}\""))?,
                );
            }
            "--supports" => a.supports = true,
            "--brim" => a.brim = true,
            "--raft" => a.raft = true,
            "--no-support-raft" => a.no_support_raft = true,
            "--ironing" => a.ironing = true,
            "--vase" => a.vase = true,
            "--fuzzy" => a.fuzzy = true,
            "--fuzzy-top" => a.fuzzy_top = true,
            "--infill-pattern" => a.infill_pattern = Some(value("--infill-pattern")?),
            "--seam" => a.seam = Some(value("--seam")?),
            s if s.starts_with('-') => return Err(format!("unknown option \"{s}\"")),
            s => a.models.push(PathBuf::from(s)),
        }
    }
    Ok(Some(a))
}

fn main() -> ExitCode {
    // Clock times (estimated completion, records) in this computer's own time zone.
    device_ui::use_system_time_zone();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("printcraft: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.slice {
        return slice_without_window(&args);
    }
    run_window(args)
}

fn run_window(args: Args) -> ExitCode {
    let capturing = args.screenshot.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PrintCraft")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([960.0, 600.0])
        // Open filling the screen; the size above is used when it's un-maximised.
        .with_maximized(true)
        .with_drag_and_drop(true);
    if capturing {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    if capturing {
        options.event_loop_builder = Some(Box::new(|builder| {
            use eframe::winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_activate_ignoring_other_apps(false);
        }));
    }
    let start = app::StartOptions {
        models: args.models,
        screenshot: args.screenshot,
        slice_on_start: args.preview,
        connect: args.connect,
        device_page: args.device,
        tool: args.tool.clone(),
        file_menu: args.file_menu,
        camera: args.camera,
    };
    match eframe::run_native(
        "PrintCraft",
        options,
        Box::new(|cc| Ok(Box::new(app::PrintCraftApp::new(cc, start)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("printcraft: could not open the window: {e}");
            ExitCode::FAILURE
        }
    }
}

fn slice_without_window(args: &Args) -> ExitCode {
    use profiles::{Adhesion, Filament, Printer, Quality};
    if args.models.is_empty() {
        eprintln!("printcraft: --slice needs at least one model (STL, OBJ or 3MF)\n\n{USAGE}");
        return ExitCode::from(2);
    }
    let Some(output) = &args.output else {
        eprintln!("printcraft: --slice needs --output OUT.gcode\n\n{USAGE}");
        return ExitCode::from(2);
    };
    let quality = match &args.quality {
        None => Quality::Standard,
        Some(q) => match Quality::parse(q) {
            Some(q) => q,
            None => {
                eprintln!("printcraft: unknown quality \"{q}\" (use fine, standard or draft)");
                return ExitCode::from(2);
            }
        },
    };
    let filaments = Filament::presets();
    let filament = match &args.filament {
        None => filaments[0].clone(),
        Some(f) => match filaments
            .iter()
            .find(|x| x.name.to_lowercase().starts_with(&f.to_lowercase()))
        {
            Some(x) => x.clone(),
            None => {
                eprintln!("printcraft: unknown filament \"{f}\" (use pla, petg, abs or tpu)");
                return ExitCode::from(2);
            }
        },
    };
    let printers = Printer::presets();
    let printer = match args.printer {
        None => printers[0].clone(),
        Some(n) if (1..=printers.len()).contains(&n) => printers[n - 1].clone(),
        Some(n) => {
            eprintln!(
                "printcraft: there is no printer {n} (use 1 to {})",
                printers.len()
            );
            return ExitCode::from(2);
        }
    };
    let mut settings = quality.settings();
    settings.supports = args.supports;
    if args.brim {
        settings.adhesion = Adhesion::Brim;
    }
    if args.raft {
        settings.adhesion = Adhesion::Raft;
    }
    settings.support_raft = !args.no_support_raft;
    settings.ironing = args.ironing;
    settings.vase_mode = args.vase;
    settings.fuzzy_skin = args.fuzzy;
    settings.fuzzy_top = args.fuzzy_top;
    if let Some(p) = &args.infill_pattern {
        match profiles::InfillPattern::ALL
            .iter()
            .find(|x| x.label().replace(' ', "-").eq_ignore_ascii_case(p))
        {
            Some(x) => settings.infill_pattern = *x,
            None => {
                eprintln!(
                    "printcraft: unknown infill pattern \"{p}\" (use lines, grid, triangles, \
                     gyroid, honeycomb, cubic, adaptive-cubic or lightning)"
                );
                return ExitCode::from(2);
            }
        }
    }
    if let Some(p) = &args.seam {
        match profiles::SeamPosition::ALL
            .iter()
            .find(|x| x.label().eq_ignore_ascii_case(p))
        {
            Some(x) => settings.seam = *x,
            None => {
                let names: Vec<String> = profiles::SeamPosition::ALL
                    .iter()
                    .map(|x| x.label().to_ascii_lowercase())
                    .collect();
                eprintln!(
                    "printcraft: unknown seam \"{p}\" (use {})",
                    names.join(", ")
                );
                return ExitCode::from(2);
            }
        }
    }

    let mut triangles = Vec::new();
    let count = args.models.len();
    for (i, path) in args.models.iter().enumerate() {
        let mesh = match model::Mesh::load(path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("printcraft: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let (mesh, fixed) = repair::repair(mesh);
        if !fixed.is_empty() {
            eprintln!("printcraft: {}: repaired: {fixed}", path.display());
        }
        let mut obj = model::PlacedObject::new(path.display().to_string(), mesh);
        // Several models: line them up left to right with a gap.
        obj.x = (i as f32 - (count as f32 - 1.0) / 2.0) * 40.0;
        if !pipeline::fits_on_plate(&obj.world_bounds(), &printer) {
            eprintln!(
                "printcraft: {} does not fit on the {} plate",
                path.display(),
                printer.name
            );
            return ExitCode::FAILURE;
        }
        triangles.extend(obj.world_triangles());
    }

    let sliced = pipeline::slice_plate(&triangles, &printer, &filament, &settings, &|_| {});
    let out = sliced.gcode;
    let wants_x3g = output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("x3g"));
    let bytes = if wants_x3g {
        if !printer.x3g {
            eprintln!(
                "printcraft: an .x3g file is for a MakerBot / Sailfish printer; the {} \
                 isn't one. Use --printer 11 (FlashForge Creator Pro).",
                printer.name
            );
            return ExitCode::from(2);
        }
        let job = output
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "print".into());
        match x3g::export(&out.text, &printer, &job, sliced.result.layers.len()) {
            Ok((bytes, warnings)) => {
                for w in warnings {
                    eprintln!("printcraft: note: {w}");
                }
                bytes
            }
            Err(e) => {
                eprintln!("printcraft: could not make the X3G file: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        out.text.clone().into_bytes()
    };
    if let Err(e) = std::fs::write(output, &bytes) {
        eprintln!("printcraft: could not write {}: {e}", output.display());
        return ExitCode::FAILURE;
    }
    let e = out.estimate;
    println!("Sliced {} model(s) into {} layers", count, e.layers);
    println!("Estimated time: {}", e.time_text());
    println!(
        "Filament: {:.2} m ({:.1} g)",
        e.filament_mm / 1000.0,
        e.grams
    );
    println!("Saved {}", output.display());
    ExitCode::SUCCESS
}
