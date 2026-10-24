use serde_json::{Map, Value};

use super::super::Format;
use super::super::emit::{render, scalar_str};
use super::super::tree::{self, Node};
use super::diff::diff_map;
use super::{
    Edits, PatchError, Patched, Reformat, Rename, end_of, entry_yaml, join_text, lead, line_start,
    own_lines, split_text,
};

pub(super) fn patch_yaml(
    text: &str,
    target: &Value,
    renames: &[Rename],
) -> Result<Patched, PatchError> {
    let (bom, eol, src) = split_text(text);
    let mut root = tree::parse(&src)
        .map_err(|e| PatchError::Parse(e.first().map(|e| e.message.clone()).unwrap_or_default()))?;
    if &root.to_json() == target && renames.is_empty() {
        return Ok(Patched {
            text: text.to_string(),
            reformat: None,
        });
    }
    let unit = indent_unit(&src);
    let mut edits = Edits {
        list: Vec::new(),
        unsupported: None,
    };
    for r in renames {
        rename_key(&src, &mut root, r, &mut edits);
    }
    let is_doc_map = matches!(root.value, tree::Value::Map(ref m) if !m.is_empty())
        && !src[root.span.start.offset..].starts_with('{');
    if is_doc_map && target.is_object() {
        diff_map(&src, &root, target, &[], 0, unit, &mut edits);
    } else {
        edits.give_up(&[], "the file is not a block mapping");
    }
    let attempt = if edits.unsupported.is_none() {
        apply(&src, &edits.list).filter(|out| verify(out, target))
    } else {
        None
    };
    if let Some(out) = attempt {
        return Ok(Patched {
            text: join_text(bom, eol, out),
            reformat: None,
        });
    }
    let reason = edits
        .unsupported
        .clone()
        .map(|(_, r)| r)
        .unwrap_or_else(|| {
            "this part of the file has a layout the patcher cannot edit in place".into()
        });
    let scope = edits
        .unsupported
        .as_ref()
        .and_then(|(p, _)| p.first().cloned())
        .map(|k| vec![k]);
    if let Some(scope) = scope
        && is_doc_map
        && let Some(out) = reformat_section(&src, &root, target, &scope[0], renames)
        && verify(&out, target)
    {
        return Ok(Patched {
            text: join_text(bom, eol, out),
            reformat: Some(Reformat { scope, reason }),
        });
    }
    let out = render(target, Format::Yaml);
    if !verify(&out, target) {
        return Err(PatchError::Verify(
            "the rewritten file does not match the wanted values".into(),
        ));
    }
    Ok(Patched {
        text: join_text(bom, eol, out),
        reformat: Some(Reformat {
            scope: Vec::new(),
            reason,
        }),
    })
}

fn verify(out: &str, target: &Value) -> bool {
    tree::parse(out).is_ok_and(|n| &n.to_json() == target) && !out.contains('\t')
}

fn apply(src: &str, edits: &[(usize, usize, String)]) -> Option<String> {
    let mut sorted: Vec<&(usize, usize, String)> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.0, e.1));
    for w in sorted.windows(2) {
        if w[0].1 > w[1].0 {
            return None;
        }
    }
    let mut out = src.to_string();
    for (from, to, with) in sorted.into_iter().rev() {
        if *to > out.len() || from > to {
            return None;
        }
        out.replace_range(*from..*to, with);
    }
    Some(out)
}

fn reformat_section(
    src: &str,
    root: &Node,
    target: &Value,
    key: &str,
    renames: &[Rename],
) -> Option<String> {
    let tree::Value::Map(entries) = &root.value else {
        return None;
    };
    let wanted = target.as_object()?;
    let renamed = renames
        .iter()
        .find(|r| r.path.len() == 1 && r.path[0] == key)
        .map_or(key, |r| r.to.as_str());
    let idx = entries.iter().position(|e| e.key == key)?;
    let start = line_start(src, entries[idx].key_span.start.offset);
    let end = match entries.get(idx + 1) {
        Some(next) => {
            let ns = line_start(src, next.key_span.start.offset);
            start + own_lines(&src[start..lead(src, ns, "")], 0)
        }
        None => end_of(src, start, entries[idx].value.span.end.offset, 0),
    };
    let text = match wanted.get(renamed) {
        Some(v) => entry_yaml(renamed, v, ""),
        None => String::new(),
    };
    let mut out = String::new();
    out.push_str(&src[..start]);
    out.push_str(&text);
    out.push_str(&src[end..]);
    let mut rest = Map::new();
    for (k, v) in wanted {
        let known = entries.iter().any(|e| e.key == *k) || k == renamed;
        if !known {
            rest.insert(k.clone(), v.clone());
        }
    }
    if !rest.is_empty() {
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render(&Value::Object(rest), Format::Yaml));
    }
    Some(out)
}

fn indent_unit(src: &str) -> usize {
    src.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(2)
        .clamp(1, 8)
}

fn rename_key(src: &str, root: &mut Node, r: &Rename, edits: &mut Edits) {
    let Some((last, parents)) = r.path.split_last() else {
        return;
    };
    let mut node = &mut *root;
    for p in parents {
        let tree::Value::Map(entries) = &mut node.value else {
            return;
        };
        let Some(e) = entries.iter_mut().find(|e| &e.key == p) else {
            return;
        };
        node = &mut e.value;
    }
    let tree::Value::Map(entries) = &mut node.value else {
        return;
    };
    let Some(e) = entries.iter_mut().find(|e| &e.key == last) else {
        return;
    };
    let span = e.key_span;
    if src[span.start.offset..span.end.offset].contains('\n') {
        edits.give_up(&r.path, "the key spans several lines");
        return;
    }
    edits.push(span.start.offset, span.end.offset, scalar_str(&r.to));
    e.key = r.to.clone();
}

pub(super) fn is_flow(src: &str, node: &Node) -> bool {
    src[node.span.start.offset..].starts_with(['{', '['])
}

pub(super) fn child(path: &[String], key: &str) -> Vec<String> {
    let mut p = path.to_vec();
    p.push(key.to_string());
    p
}

pub(super) fn entry_bounds(
    src: &str,
    entries: &[tree::Entry],
    indent: &str,
) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = entries
        .iter()
        .map(|e| line_start(src, e.key_span.start.offset))
        .collect();
    (0..entries.len())
        .map(|i| {
            let end = match starts.get(i + 1) {
                Some(&next) => {
                    let to = lead(src, next, indent).max(starts[i]);
                    starts[i] + own_lines(&src[starts[i]..to], indent.len())
                }
                None => end_of(
                    src,
                    starts[i],
                    entries[i]
                        .value
                        .span
                        .end
                        .offset
                        .max(entries[i].key_span.end.offset),
                    indent.len(),
                ),
            };
            (starts[i], end)
        })
        .collect()
}
