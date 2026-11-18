//! Colour changes and pauses at chosen heights, set in the preview and saved in the
//! project. They are added to the finished G-code text, right after the `;LAYER:` marker
//! of the first layer at or above their height, so changing them needs no new slice.
//!
//! The commands: a colour change is `M600` (Marlin, Prusa firmware, and Klipper set-ups
//! that define an `M600` macro); a pause is `PAUSE` on Klipper and `M601` elsewhere.
//! MakerBot / Sailfish (X3G) printers have neither, so the preview doesn't offer them.

use crate::profiles::Printer;

/// What happens before a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerEventKind {
    /// Stop for a filament swap (`M600`), then carry on.
    ColourChange,
    /// Stop until the person resumes (`M601`, or `PAUSE` on Klipper).
    Pause,
}

impl LayerEventKind {
    /// The name in a project file.
    pub fn key(self) -> &'static str {
        match self {
            Self::ColourChange => "colour_change",
            Self::Pause => "pause",
        }
    }

    /// The kind with project-file name `key`.
    pub fn from_key(key: &str) -> Option<Self> {
        [Self::ColourChange, Self::Pause]
            .into_iter()
            .find(|k| k.key() == key)
    }

    /// How it is named in the preview.
    pub fn label(self) -> &'static str {
        match self {
            Self::ColourChange => "Colour change",
            Self::Pause => "Pause",
        }
    }

    /// The G-code line for this printer.
    fn line(self, printer: &Printer) -> &'static str {
        match (self, printer.klipper) {
            (Self::ColourChange, _) => "M600 ; colour change",
            (Self::Pause, true) => "PAUSE ; pause",
            (Self::Pause, false) => "M601 ; pause",
        }
    }
}

/// A colour change or pause before the first layer at or above height `z` (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerEvent {
    pub z: f32,
    pub kind: LayerEventKind,
}

/// The index of the first layer (heights `layer_zs`, in order) at or above `z`, allowing
/// for rounding; `None` above the print.
pub fn layer_for(z: f32, layer_zs: &[f32]) -> Option<usize> {
    layer_zs.iter().position(|lz| *lz >= z - 1e-3)
}

/// `text` with each event's command added after its layer's `;LAYER:` line. Events on
/// one layer keep their order; events above the print are left out.
pub fn add_layer_events(
    text: &str,
    layer_zs: &[f32],
    events: &[LayerEvent],
    printer: &Printer,
) -> String {
    if events.is_empty() {
        return text.to_string();
    }
    let at: Vec<(usize, LayerEventKind)> = events
        .iter()
        .filter_map(|e| layer_for(e.z, layer_zs).map(|li| (li, e.kind)))
        .collect();
    let mut out = String::with_capacity(text.len() + 32 * at.len());
    for line in text.split_inclusive('\n') {
        out.push_str(line);
        let Some(li) = line
            .trim_end()
            .strip_prefix(";LAYER:")
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        for (_, kind) in at.iter().filter(|(l, _)| *l == li) {
            out.push_str(kind.line(printer));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Printer;

    fn marlin() -> Printer {
        Printer::presets()[0].clone()
    }

    fn klipper() -> Printer {
        Printer::presets().into_iter().find(|p| p.klipper).unwrap()
    }

    const TEXT: &str = "G28\n;LAYER:0\nG1 Z0.2\n;LAYER:1\nG1 Z0.4\n;LAYER:2\nG1 Z0.6\n;END\n";
    const ZS: [f32; 3] = [0.2, 0.4, 0.6];

    #[test]
    fn test_layer_event_written_after_its_layer_marker() {
        let events = [LayerEvent {
            z: 0.4,
            kind: LayerEventKind::ColourChange,
        }];
        let out = add_layer_events(TEXT, &ZS, &events, &marlin());
        assert_eq!(
            out,
            "G28\n;LAYER:0\nG1 Z0.2\n;LAYER:1\nM600 ; colour change\nG1 Z0.4\n;LAYER:2\nG1 Z0.6\n;END\n"
        );
    }

    #[test]
    fn test_layer_event_pause_command_follows_the_firmware() {
        let pause = [LayerEvent {
            z: 0.6,
            kind: LayerEventKind::Pause,
        }];
        assert!(add_layer_events(TEXT, &ZS, &pause, &marlin()).contains(";LAYER:2\nM601 ; pause\n"));
        assert!(
            add_layer_events(TEXT, &ZS, &pause, &klipper()).contains(";LAYER:2\nPAUSE ; pause\n")
        );
    }

    #[test]
    fn test_layer_event_lands_on_the_first_layer_at_or_above_its_height() {
        // A height between layers (after a layer-height change) goes to the next layer up.
        let between = [LayerEvent {
            z: 0.3,
            kind: LayerEventKind::Pause,
        }];
        assert!(add_layer_events(TEXT, &ZS, &between, &marlin()).contains(";LAYER:1\nM601"));
        // Above the print: nothing is written.
        let above = [LayerEvent {
            z: 9.0,
            kind: LayerEventKind::Pause,
        }];
        assert_eq!(add_layer_events(TEXT, &ZS, &above, &marlin()), TEXT);
        // No events: the text as it was.
        assert_eq!(add_layer_events(TEXT, &ZS, &[], &marlin()), TEXT);
    }

    #[test]
    fn test_layer_events_two_on_one_layer_keep_their_order() {
        let events = [
            LayerEvent {
                z: 0.4,
                kind: LayerEventKind::Pause,
            },
            LayerEvent {
                z: 0.4,
                kind: LayerEventKind::ColourChange,
            },
        ];
        let out = add_layer_events(TEXT, &ZS, &events, &marlin());
        assert!(
            out.contains(";LAYER:1\nM601 ; pause\nM600 ; colour change\n"),
            "{out}"
        );
    }

    #[test]
    fn test_layer_event_layer_index() {
        assert_eq!(layer_for(0.4, &ZS), Some(1));
        assert_eq!(layer_for(0.40001, &ZS), Some(1));
        assert_eq!(layer_for(0.0, &ZS), Some(0));
        assert_eq!(layer_for(1.0, &ZS), None);
    }
}
