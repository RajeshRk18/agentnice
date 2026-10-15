//! Quota usage. agentnice keeps it in the same file and format as abtop,
//! so both tools read one copy.

use serde_json::{Value, json};
use std::{fs, io, path::Path};

pub const WINDOWS: [&str; 2] = ["five_hour", "seven_day"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub name: &'static str,
    /// Percent used, 0 to 100.
    pub used: f64,
    /// Unix seconds.
    pub resets_at: u64,
}

/// Takes the known windows from a `rate_limits` object.
/// A window that is missing or has a bad field is left out.
pub fn parse(v: &Value) -> Vec<Window> {
    WINDOWS
        .iter()
        .filter_map(|&name| {
            let w = v.get(name)?;
            Some(Window {
                name,
                used: w.get("used_percentage")?.as_f64()?,
                resets_at: w.get("resets_at")?.as_f64()? as u64,
            })
        })
        .collect()
}

/// Writes the file. It writes a temp file and renames it, so a reader
/// never sees half a file.
pub fn write(path: &Path, windows: &[Window], now: u64) -> io::Result<()> {
    let mut v = json!({ "source": "claude", "updated_at": now });
    for w in windows {
        v[w.name] = json!({ "used_percentage": w.used, "resets_at": w.resets_at });
    }
    let tmp = path.with_extension("json.agentnice-tmp");
    fs::write(&tmp, v.to_string())
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_statusline_shape_and_skips_unknown_windows() {
        let v: Value = serde_json::from_str(
            r#"{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},
                "seven_day":{"used_percentage":41.2},
                "spend_limit":{"used_percentage":62.8,"resets_at":1740787200}}"#,
        )
        .unwrap();
        let w = parse(&v);
        assert_eq!(
            w,
            [Window {
                name: "five_hour",
                used: 23.5,
                resets_at: 1738425600
            }]
        );
    }
}
