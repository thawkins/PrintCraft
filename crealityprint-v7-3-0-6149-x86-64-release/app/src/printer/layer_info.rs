//! Telling a Klipper-based printer (Ender-3 V3 KE, Moonraker printers) how many layers
//! a print has, so its screen and status show "layer 9 of 150" instead of "of 0".
//!
//! SET_PRINT_STATS_INFO is Klipper's documented command for this. Printers that don't
//! know it just report "unknown command" and carry on, but it is only added to files
//! sent to Klipper-based printers.

/// Number of layers in a print file (counts `;LAYER:` markers).
pub fn layer_count(gcode: &str) -> usize {
    gcode.lines().filter(|l| l.starts_with(";LAYER:")).count()
}

/// The file with the total layer count set at the start and the current layer set at
/// every layer change. Files that already set them are returned unchanged.
pub fn with_layer_info(gcode: &str) -> String {
    if gcode.contains("SET_PRINT_STATS_INFO") {
        return gcode.to_string();
    }
    let total = layer_count(gcode);
    if total == 0 {
        return gcode.to_string();
    }
    let mut out = String::with_capacity(gcode.len() + total * 40 + 64);
    let mut announced = false;
    for line in gcode.split_inclusive('\n') {
        if let Some(n) = line.trim_end().strip_prefix(";LAYER:") {
            if !announced {
                out.push_str(&format!("SET_PRINT_STATS_INFO TOTAL_LAYER={total}\n"));
                announced = true;
            }
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
            }
            if let Ok(i) = n.trim().parse::<usize>() {
                out.push_str(&format!(
                    "SET_PRINT_STATS_INFO CURRENT_LAYER={}\n",
                    i.saturating_add(1)
                ));
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_total_and_each_layer() {
        let g = "G28\n;LAYER:0\nG1 X1\n;LAYER:1\nG1 X2\n;END\n";
        let out = with_layer_info(g);
        assert!(out.contains(
            "SET_PRINT_STATS_INFO TOTAL_LAYER=2\n;LAYER:0\nSET_PRINT_STATS_INFO CURRENT_LAYER=1\n"
        ));
        assert!(out.contains(";LAYER:1\nSET_PRINT_STATS_INFO CURRENT_LAYER=2\n"));
        assert_eq!(with_layer_info(&out), out, "added only once");
        assert_eq!(layer_count(g), 2);
    }
}
