use std::collections::BTreeMap;

const CSS: &str = include_str!("../style/entry.css");

fn block(css: &str, start: &str) -> String {
    let at = css
        .find(start)
        .unwrap_or_else(|| panic!("{start} is in entry.css"));
    let open = at + css[at..].find('{').expect("a block");
    let mut depth = 0;
    for (i, ch) in css[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return css[open + 1..open + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("{start} is not closed")
}

fn tokens(body: &str) -> BTreeMap<String, String> {
    body.split(';')
        .filter_map(|d| d.trim().strip_prefix("--color-"))
        .filter_map(|d| d.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn hex(v: &str) -> Option<[f64; 3]> {
    let h = v.strip_prefix('#')?;
    let n = u32::from_str_radix(h, 16).ok()?;
    (h.len() == 6).then_some([(n >> 16) as f64, ((n >> 8) & 255) as f64, (n & 255) as f64])
}

fn luminance(c: [f64; 3]) -> f64 {
    let lin = |v: f64| {
        let s = v / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

pub fn ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn modes() -> Vec<(&'static str, BTreeMap<String, String>)> {
    let light = tokens(&block(CSS, "@theme"));
    let mut dark = light.clone();
    dark.extend(tokens(&block(CSS, ":root.dark")));
    vec![("light", light), ("dark", dark)]
}

const TEXT: &[(&str, &str)] = &[
    ("text", "bg"),
    ("text", "surface"),
    ("text", "surface-2"),
    ("text-muted", "bg"),
    ("text-muted", "surface"),
    ("text-muted", "surface-2"),
    ("accent-ink", "bg"),
    ("accent-ink", "surface"),
    ("code-text", "code"),
    ("hdr-text", "hdr"),
    ("hdr-muted", "hdr"),
    ("hdr-on", "hdr"),
    ("tip-text", "tip"),
];

#[test]
fn body_text_meets_aa_on_every_surface_in_both_themes() {
    let mut bad = Vec::new();
    for (mode, t) in modes() {
        for (fg, bg) in TEXT {
            let (Some(f), Some(b)) = (
                t.get(*fg).and_then(|v| hex(v)),
                t.get(*bg).and_then(|v| hex(v)),
            ) else {
                panic!("{mode}: {fg} or {bg} is not a #rrggbb token");
            };
            let r = ratio(f, b);
            if r < 4.5 {
                bad.push(format!("{mode}: {fg} on {bg} is {r:.2}:1"));
            }
        }
    }
    assert!(bad.is_empty(), "below 4.5:1: {bad:#?}");
}

#[test]
fn the_dark_theme_is_the_same_for_the_class_and_the_system_setting() {
    let by_class = tokens(&block(CSS, ":root.dark"));
    let by_system = tokens(&block(CSS, ":root:not(.light)"));
    assert_eq!(by_class, by_system);
}

#[test]
fn the_ratio_matches_known_values() {
    assert!((ratio([0.0; 3], [255.0; 3]) - 21.0).abs() < 1e-9);
    assert!((ratio([255.0; 3], [255.0; 3]) - 1.0).abs() < 1e-9);
}
