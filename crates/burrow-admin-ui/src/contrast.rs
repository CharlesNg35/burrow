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
    ("text", "surface-3"),
    ("text-muted", "bg"),
    ("text-muted", "surface"),
    ("text-muted", "surface-2"),
    ("text-muted", "surface-3"),
    ("text-faint", "bg"),
    ("text-faint", "surface"),
    ("text-faint", "surface-2"),
    ("accent-ink", "bg"),
    ("accent-ink", "surface"),
    ("accent-ink", "surface-2"),
    ("accent-ink", "accent-weak"),
    ("on-accent", "accent"),
    ("on-accent", "accent:hover"),
    ("success", "bg"),
    ("success", "surface"),
    ("success", "surface-2"),
    ("warning", "bg"),
    ("warning", "surface"),
    ("warning", "surface-2"),
    ("danger", "bg"),
    ("danger", "surface"),
    ("danger", "surface-2"),
    ("danger", "danger-weak"),
    ("danger-ink", "danger"),
    ("danger-ink", "danger:hover"),
    ("warning-ink", "warning"),
    ("co-warning", "warning-weak"),
    ("co-danger", "danger-weak"),
    ("co-success", "success-weak"),
    ("co-info", "accent-weak"),
    ("code-text", "code"),
    ("hdr-text", "hdr"),
    ("hdr-text", "hdr-raised"),
    ("hdr-muted", "hdr"),
    ("hdr-on", "hdr"),
    ("hdr-on", "tab-on"),
    ("tip-text", "tip"),
    ("text-faint", "surface-3"),
    ("text-muted", "accent-weak"),
    ("text-faint", "accent-weak"),
    ("hdr-muted", "hdr-raised"),
    ("hdr-muted", "hdr-raised-2"),
    ("success", "success-weak"),
    ("warning", "warning-weak"),
];

const ON_PAGE: &[(&str, &str)] = &[
    ("success@bg", "success-weak@bg"),
    ("warning@bg", "warning-weak@bg"),
    ("danger@bg", "danger-weak@bg"),
    ("accent-ink@bg", "accent-weak@bg"),
    ("udp@bg", "udp-weak@bg"),
    ("udp@surface", "udp-weak@surface"),
    ("success@surface-2", "success-weak@surface-2"),
    ("danger@surface-2", "danger-weak@surface-2"),
    ("accent-ink@surface-2", "accent-weak@surface-2"),
];

const UI: &[(&str, &str)] = &[
    ("border-strong", "surface"),
    ("border-strong", "input"),
    ("accent", "surface"),
    ("accent", "bg"),
    ("border-strong", "bg"),
    ("border-strong", "surface-2"),
];

fn rgba(v: &str) -> Option<([f64; 3], f64)> {
    if let Some(c) = hex(v) {
        return Some((c, 1.0));
    }
    let inner = v.strip_prefix("rgb(")?.strip_suffix(')')?;
    let (rgb, alpha) = inner.split_once('/')?;
    let p: Vec<f64> = rgb
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    let a: f64 = alpha.trim().parse().ok()?;
    (p.len() == 3).then_some(([p[0], p[1], p[2]], a))
}

fn color(t: &BTreeMap<String, String>, name: &str, mode: &str) -> [f64; 3] {
    let (name, base) = name.split_once('@').unwrap_or((name, "surface"));
    let (token, hover) = match name.split_once(':') {
        Some((n, _)) => (n, true),
        None => (name, false),
    };
    let under = rgba(&t[base]).expect("the base is a color").0;
    let (c, a) = t
        .get(token)
        .and_then(|v| rgba(v))
        .unwrap_or_else(|| panic!("{mode}: {token} is not a color token"));
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = c[i] * a + under[i] * (1.0 - a);
    }
    if hover {
        let k = if mode == "light" { 0.95 } else { 1.1 };
        for v in &mut out {
            *v = (*v * k).round().min(255.0);
        }
    }
    out
}

fn failures(pairs: &[(&str, &str)], need: f64) -> Vec<String> {
    let mut bad = Vec::new();
    for (mode, t) in modes() {
        for (fg, bg) in pairs {
            let r = ratio(color(&t, fg, mode), color(&t, bg, mode));
            if r < need {
                bad.push(format!("{mode}: {fg} on {bg} is {r:.2}:1"));
            }
        }
    }
    bad
}

#[test]
fn every_text_pair_meets_aa_in_both_themes() {
    let bad = failures(TEXT, 4.5);
    assert!(bad.is_empty(), "below 4.5:1: {bad:#?}");
}

#[test]
fn tags_and_tints_on_the_grey_page_meet_aa_in_both_themes() {
    let bad = failures(ON_PAGE, 4.5);
    assert!(bad.is_empty(), "below 4.5:1: {bad:#?}");
}

#[test]
fn borders_and_the_accent_meet_three_to_one_in_both_themes() {
    let bad = failures(UI, 3.0);
    assert!(bad.is_empty(), "below 3:1: {bad:#?}");
}

#[test]
fn hover_darkens_solid_buttons_in_light_and_brightens_them_in_dark() {
    let kit = include_str!("../../burrow-ui-kit/src/basic.rs");
    assert!(!kit.contains(" hover:brightness-110"));
    assert!(kit.contains("hover:brightness-95 dark:hover:brightness-110"));
}

#[test]
fn the_dark_theme_is_the_same_for_the_class_and_the_system_setting() {
    let by_class = tokens(&block(CSS, ":root.dark"));
    let by_system = tokens(&block(CSS, ":root:not(.light) {"));
    assert_eq!(by_class, by_system);
}

#[test]
fn the_ratio_matches_known_values() {
    assert!((ratio([0.0; 3], [255.0; 3]) - 21.0).abs() < 1e-9);
    assert!((ratio([255.0; 3], [255.0; 3]) - 1.0).abs() < 1e-9);
}

#[test]
fn the_one_tooltip_uses_the_tip_colours_in_both_themes() {
    let kit = include_str!("../../burrow-ui-kit/src/overlay.rs");
    assert!(kit.contains("bg-tip") && kit.contains("text-tip-text"));
    let flow = include_str!("screens/flow/view.rs");
    assert!(flow.contains("<Tooltip") && !flow.contains("bg-tip"));
}
