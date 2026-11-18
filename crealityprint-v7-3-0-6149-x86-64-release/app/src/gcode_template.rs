//! Placeholders in custom G-code: `{name}` is replaced by a value, `{name-1}` /
//! `{name+5}` by the value plus or minus a number, and `{if a op b}` ... `{else}` ...
//! `{endif}` keeps one branch (`op` is `==`, `!=`, `<`, `<=`, `>` or `>=`; `a` and `b` are
//! names or numbers; `{if name}` is true when the value isn't 0). Branches can nest.
//! `{{` and `}}` write a literal brace. Anything else is copied as written.
//!
//! Values come from a lookup function, so the renderer knows nothing about printers. A
//! name the lookup doesn't know is an error, even in a branch that is skipped (it is a
//! typo either way), and so is a malformed tag; the error names the tag.

/// A tag's text as written, for error messages.
fn tag(inner: &str) -> String {
    format!("{{{inner}}}")
}

/// Writes a value without trailing zeros: `60`, `0.2`, `-1.25` (at most 3 decimals).
pub fn format_number(v: f64) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// The value of a name or a number literal.
fn operand(word: &str, vars: &dyn Fn(&str) -> Option<f64>, whole: &str) -> Result<f64, String> {
    let word = word.trim();
    if let Ok(n) = word.parse::<f64>() {
        return Ok(n);
    }
    vars(word).ok_or_else(|| format!("unknown placeholder {}", tag(whole)))
}

/// `name`, a number, or `name + number` / `name - number`.
fn expression(inner: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
    let text = inner.trim();
    // The last + or - that isn't the expression's own sign splits off the offset.
    let split = text
        .char_indices()
        .rev()
        .find(|(i, c)| *i > 0 && (*c == '+' || *c == '-'));
    match split {
        Some((i, sign)) => {
            let base = operand(&text[..i], vars, inner)?;
            let offset: f64 = text[i + 1..]
                .trim()
                .parse()
                .map_err(|_| format!("{} needs a number after {sign}", tag(inner)))?;
            Ok(if sign == '+' {
                base + offset
            } else {
                base - offset
            })
        }
        None => operand(text, vars, inner),
    }
}

/// The condition of an `{if ...}` tag (`cond` is the text after `if`).
fn condition(cond: &str, inner: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<bool, String> {
    const OPS: [&str; 6] = ["==", "!=", "<=", ">=", "<", ">"];
    let found = OPS
        .iter()
        .filter_map(|op| cond.find(op).map(|at| (at, *op)))
        .min_by_key(|(at, op)| (*at, std::cmp::Reverse(op.len())));
    match found {
        Some((at, op)) => {
            let (a, b) = (&cond[..at], &cond[at + op.len()..]);
            if OPS.iter().any(|o| b.trim_start().starts_with(o)) || a.trim().is_empty() {
                return Err(format!(
                    "{} is not a comparison PrintCraft understands",
                    tag(inner)
                ));
            }
            let (x, y) = (operand(a, vars, inner)?, operand(b, vars, inner)?);
            Ok(match op {
                "==" => x == y,
                "!=" => x != y,
                "<=" => x <= y,
                ">=" => x >= y,
                "<" => x < y,
                _ => x > y,
            })
        }
        None => Ok(operand(cond, vars, inner)? != 0.0),
    }
}

/// One open `{if}`: whether its enclosing text is written, its condition, and whether
/// its `{else}` has been seen.
struct Branch {
    outer: bool,
    cond: bool,
    in_else: bool,
    tag: String,
}

impl Branch {
    fn writing(&self) -> bool {
        self.outer && (self.cond != self.in_else)
    }
}

/// `text` with its placeholders filled in from `vars`.
pub fn render(text: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut open: Vec<Branch> = Vec::new();
    let writing = |open: &[Branch]| open.last().is_none_or(Branch::writing);
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        if writing(&open) {
            out.push_str(&rest[..at]);
        }
        let brace = &rest[at..];
        if brace.starts_with("{{") || brace.starts_with("}}") {
            if writing(&open) {
                out.push_str(&brace[..1]);
            }
            rest = &brace[2..];
            continue;
        }
        if brace.starts_with('}') {
            return Err("a } has no { before it (write }} for a literal brace)".to_string());
        }
        let Some(close) = brace.find('}') else {
            let line = brace.lines().next().unwrap_or(brace);
            return Err(format!(
                "{line} is not closed with }} (write {{{{ for a literal brace)"
            ));
        };
        let inner = &brace[1..close];
        rest = &brace[close + 1..];
        let word = inner.trim();
        if let Some(cond) = word.strip_prefix("if ") {
            let cond = condition(cond, inner, vars)?;
            let outer = writing(&open);
            open.push(Branch {
                outer,
                cond,
                in_else: false,
                tag: tag(inner),
            });
        } else if word == "else" {
            match open.last_mut() {
                Some(b) if !b.in_else => b.in_else = true,
                Some(b) => return Err(format!("{} has two {{else}}", b.tag)),
                None => return Err("{else} without an {if ...} before it".to_string()),
            }
        } else if word == "endif" {
            if open.pop().is_none() {
                return Err("{endif} without an {if ...} before it".to_string());
            }
        } else {
            let value = expression(inner, vars)?;
            if writing(&open) {
                out.push_str(&format_number(value));
            }
        }
    }
    if writing(&open) {
        out.push_str(rest);
    }
    match open.last() {
        Some(b) => Err(format!("{} has no {{endif}}", b.tag)),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str) -> Option<f64> {
        match name {
            "bed_temp" => Some(60.0),
            "nozzle_temp" => Some(210.0),
            "layer_z" => Some(0.4),
            "layer_num" => Some(2.0),
            "zero" => Some(0.0),
            _ => None,
        }
    }

    #[test]
    fn test_template_substitutes_values_and_offsets() {
        let out = render(
            "M140 S{bed_temp}\nMIN={bed_temp-1} Z{layer_z} N{layer_num+3}",
            &vars,
        );
        assert_eq!(out.unwrap(), "M140 S60\nMIN=59 Z0.4 N5");
        // Plain text, and doubled braces for literal ones, pass through.
        assert_eq!(render("G28 ; {{home}}", &vars).unwrap(), "G28 ; {home}");
    }

    #[test]
    fn test_template_if_else_and_nesting() {
        let t = "{if bed_temp > 50}hot{else}cold{endif}";
        assert_eq!(render(t, &vars).unwrap(), "hot");
        let t = "{if nozzle_temp <= 200}A{else}B{endif}";
        assert_eq!(render(t, &vars).unwrap(), "B");
        let t = "{if zero}yes{else}no{endif}|{if layer_num}yes{endif}";
        assert_eq!(render(t, &vars).unwrap(), "no|yes");
        let t = "{if layer_num == 2}{if bed_temp != 60}x{else}y{endif}z{endif}";
        assert_eq!(render(t, &vars).unwrap(), "yz");
        // A false branch is skipped, placeholders and all.
        assert_eq!(render("{if zero}{bed_temp}{endif}.", &vars).unwrap(), ".");
    }

    #[test]
    fn test_template_errors_name_the_problem() {
        let err = |t: &str| render(t, &vars).unwrap_err();
        assert!(
            err("M104 S{nozle_temp}").contains("{nozle_temp}"),
            "{}",
            err("M104 S{nozle_temp}")
        );
        assert!(err("{if bed_temp > 50}x").contains("{endif}"));
        assert!(err("x{endif}").contains("{endif}"));
        assert!(err("x{else}y").contains("{else}"));
        assert!(err("M104 S{bed_temp").contains("not closed"));
        assert!(err("{if bed_temp >< 3}x{endif}").contains("{if bed_temp >< 3}"));
        // An unknown name in a skipped branch is still an error: it is a typo either way.
        assert!(err("{if zero}{nozle_temp}{endif}").contains("{nozle_temp}"));
    }

    #[test]
    fn test_template_numbers_are_short() {
        assert_eq!(format_number(60.0), "60");
        assert_eq!(format_number(0.2), "0.2");
        assert_eq!(format_number(0.30000001), "0.3");
        assert_eq!(format_number(-1.25), "-1.25");
    }
}
