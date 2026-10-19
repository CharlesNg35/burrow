#![no_main]

use burrow::config::patch::{patch, split_text};
use burrow::config::{Format, tree};
use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};

fn mutate(v: &mut Value, ops: &[u8]) {
    let mut cur = ops.iter();
    while let (Some(&what), Some(&n)) = (cur.next(), cur.next()) {
        let Some(map) = v.as_object_mut() else {
            return;
        };
        let keys: Vec<String> = map.keys().cloned().collect();
        let key = keys
            .get(n as usize % keys.len().max(1))
            .cloned()
            .unwrap_or_else(|| format!("k{n}"));
        match what % 7 {
            0 => {
                map.shift_remove(&key);
            }
            1 => {
                map.insert(format!("k{n}"), json!(n));
            }
            2 => {
                map.insert(key, json!(format!("v{n}")));
            }
            3 => {
                map.insert(key, json!([n, format!("x{n}")]));
            }
            4 => {
                map.insert(key, json!({ "a": n, "b": [format!("${{V{n}}}")] }));
            }
            5 => {
                if let Some(Value::Array(a)) = map.get_mut(&key)
                    && !a.is_empty()
                {
                    let at = n as usize % a.len();
                    a.remove(at);
                }
            }
            _ => {
                if let Some(Value::Array(a)) = map.get_mut(&key)
                    && a.len() == 2
                {
                    let only = a.remove(n as usize % 2);
                    map.insert(key, only);
                }
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let (text, ops) = data.split_at(split);
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    for format in [Format::Yaml, Format::Json] {
        let parsed = match format {
            Format::Yaml => tree::parse(&split_text(text).2).ok().map(|n| n.to_json()),
            Format::Json => serde_json::from_str::<Value>(&split_text(text).2).ok(),
        };
        let Some(mut target) = parsed else {
            continue;
        };
        mutate(&mut target, ops);
        if let Ok(out) = patch(text, format, &target, &[]) {
            let back = match format {
                Format::Yaml => tree::parse(&split_text(&out.text).2).ok().map(|n| n.to_json()),
                Format::Json => serde_json::from_str::<Value>(&split_text(&out.text).2).ok(),
            };
            assert_eq!(back.as_ref(), Some(&target), "{}", out.text);
        }
    }
});
