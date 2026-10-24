use std::path::PathBuf;

use serde_json::Value;

pub fn baselines_dir() -> PathBuf {
    std::env::var_os("BURROW_E2E_BASELINES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/admin_e2e/baselines")
        })
}

pub enum Baseline {
    Same,
    Written,
    Skipped(String),
    Differs { ratio: f64, diff: Vec<u8> },
}

const CHANNEL_TOLERANCE: u8 = 24;
const MAX_RATIO: f64 = 0.005;

fn major(v: &str) -> &str {
    v.split('.').next().unwrap_or(v)
}

pub fn compare(name: &str, png: &[u8], chrome_version: &str) -> Baseline {
    let dir = baselines_dir();
    let file = dir.join(format!("{name}.png"));
    let stamp = dir.join("chrome-version.txt");
    if std::env::var("BURROW_E2E_UPDATE_BASELINES").as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("a baselines folder");
        std::fs::write(&file, png).expect("write a baseline");
        let recorded = std::fs::read_to_string(&stamp).unwrap_or_default();
        assert_eq!(
            major(recorded.trim()),
            major(chrome_version),
            "baselines are recorded with the pinned Chrome {}; pin the new version in xtask/src/tools.rs and chrome-version.txt first",
            recorded.trim()
        );
        return Baseline::Written;
    }
    if std::env::var("BURROW_E2E_SKIP_BASELINES").as_deref() == Ok("1") {
        return Baseline::Skipped("BURROW_E2E_SKIP_BASELINES=1".into());
    }
    let recorded = std::fs::read_to_string(&stamp).unwrap_or_default();
    assert_eq!(
        major(recorded.trim()),
        major(chrome_version),
        "baselines were recorded with Chrome {}, this is Chrome {chrome_version}: run cargo xtask tools to get the pinned Chrome (BURROW_E2E_SKIP_BASELINES=1 skips the comparison)",
        recorded.trim()
    );
    let Ok(old) = std::fs::read(&file) else {
        return Baseline::Skipped(format!("no baseline {}", file.display()));
    };
    let a = image::load_from_memory(&old)
        .expect("a baseline PNG")
        .to_rgba8();
    let b = image::load_from_memory(png)
        .expect("a screenshot PNG")
        .to_rgba8();
    if a.dimensions() != b.dimensions() {
        return Baseline::Differs {
            ratio: 1.0,
            diff: png.to_vec(),
        };
    }
    let (w, h) = a.dimensions();
    let mut diff = image::RgbaImage::new(w, h);
    let mut bad = 0u64;
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let off =
            pa.0.iter()
                .zip(pb.0.iter())
                .any(|(p, q)| p.abs_diff(*q) > CHANNEL_TOLERANCE);
        if off {
            bad += 1;
            diff.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
        } else {
            let g = (u16::from(pa.0[0]) + u16::from(pa.0[1]) + u16::from(pa.0[2])) / 3;
            let g = (g / 3 + 170) as u8;
            diff.put_pixel(x, y, image::Rgba([g, g, g, 255]));
        }
    }
    let ratio = bad as f64 / (u64::from(w) * u64::from(h)).max(1) as f64;
    if ratio <= MAX_RATIO {
        return Baseline::Same;
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(diff)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode the diff");
    Baseline::Differs { ratio, diff: out }
}

fn s(v: &Value) -> String {
    v.get("value")
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn prop(n: &Value, name: &str) -> Option<String> {
    n.get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|p| p.get("value"))
        .map(s)
}

const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "checkbox",
    "combobox",
    "listbox",
    "menuitem",
    "radio",
    "switch",
    "tab",
    "searchbox",
    "spinbutton",
    "slider",
];

pub fn ax_problems(nodes: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    let mut dialogs = 0;
    for n in nodes {
        if n.get("ignored").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let role = n.get("role").map(s).unwrap_or_default();
        let name = n.get("name").map(s).unwrap_or_default();
        if INTERACTIVE.contains(&role.as_str()) && name.trim().is_empty() {
            out.push(format!(
                "a {role} has no accessible name (node {})",
                n["nodeId"]
            ));
        }
        if prop(n, "invalid").is_some_and(|v| v != "false") {
            let described = n.get("description").map(s).unwrap_or_default();
            if described.trim().is_empty() {
                out.push(format!("invalid {role} {name:?} has no error description"));
            }
        }
        if role == "dialog" || role == "alertdialog" {
            dialogs += 1;
            if prop(n, "modal").as_deref() != Some("true") {
                out.push(format!("dialog {name:?} is not modal"));
            }
        }
        if role == "image" && name.trim().is_empty() {
            out.push(format!(
                "an image has no text alternative (node {})",
                n["nodeId"]
            ));
        }
    }
    if dialogs > 1 {
        out.push(format!("{dialogs} dialogs are open at once"));
    }
    out
}

pub fn ax_names(nodes: &[Value], role: &str) -> Vec<String> {
    nodes
        .iter()
        .filter(|n| n.get("ignored").and_then(Value::as_bool) != Some(true))
        .filter(|n| n.get("role").map(s).as_deref() == Some(role))
        .map(|n| n.get("name").map(s).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(role: &str, name: &str, props: Value) -> Value {
        json!({"nodeId": "1", "ignored": false, "role": {"type": "role", "value": role}, "name": {"type": "computedString", "value": name}, "properties": props})
    }

    #[test]
    fn unnamed_controls_and_silent_errors_are_reported() {
        let ok = node("button", "Apply", json!([]));
        let unnamed = node("button", "", json!([]));
        let invalid = node(
            "textbox",
            "Port",
            json!([{"name": "invalid", "value": {"type": "token", "value": "true"}}]),
        );
        let dialog = node(
            "dialog",
            "Review",
            json!([{"name": "modal", "value": {"type": "boolean", "value": true}}]),
        );
        assert!(ax_problems(&[ok, dialog]).is_empty());
        let p = ax_problems(&[unnamed, invalid]);
        assert_eq!(p.len(), 2, "{p:?}");
    }
}
