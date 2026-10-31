use serde_json::Value;

use super::Format;

pub fn render(value: &Value, format: Format) -> String {
    match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(value).expect("json value serializes");
            s.push('\n');
            s
        }
        Format::Yaml => {
            let mut out = String::new();
            yaml(value, 0, &mut out);
            out
        }
    }
}

fn yaml(value: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.push_str(&pad);
                out.push_str(&scalar_str(k));
                out.push(':');
                match v {
                    Value::Object(m) if !m.is_empty() => {
                        out.push('\n');
                        yaml(v, indent + 1, out);
                    }
                    Value::Array(a) if !a.is_empty() => {
                        out.push('\n');
                        yaml(v, indent + 1, out);
                    }
                    _ => {
                        out.push(' ');
                        out.push_str(&inline(v));
                        out.push('\n');
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(&flow(item));
                out.push('\n');
            }
        }
        other => {
            out.push_str(&pad);
            out.push_str(&inline(other));
            out.push('\n');
        }
    }
}

pub(crate) fn flow(v: &Value) -> String {
    match v {
        Value::Object(m) if !m.is_empty() => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", scalar_str(k), flow(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Array(a) if !a.is_empty() => {
            let parts: Vec<String> = a.iter().map(flow).collect();
            format!("[{}]", parts.join(", "))
        }
        other => inline(other),
    }
}

pub(crate) fn inline(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => scalar_str(s),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Object(m) if m.is_empty() => "{}".into(),
        other => serde_json::to_string(other).expect("json value serializes"),
    }
}

pub(crate) fn scalar_str(s: &str) -> String {
    let plain_ok = !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+'))
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '=' | '-' | ':' | '@')
        })
        && !s.ends_with(':')
        && !matches!(
            s,
            "true"
                | "false"
                | "True"
                | "False"
                | "TRUE"
                | "FALSE"
                | "null"
                | "Null"
                | "NULL"
                | "~"
                | "yes"
                | "no"
                | "on"
                | "off"
        )
        && s.parse::<f64>().is_err()
        && s.parse::<i64>().is_err();
    if plain_ok {
        s.to_string()
    } else {
        serde_json::to_string(s).expect("string serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_in_a_list_is_a_yaml_flow_map() {
        let v = serde_json::json!({"clients": ["home", {"name": "gw", "target": "10.0.0.12:3000", "weight": 2}]});
        assert_eq!(
            render(&v, Format::Yaml),
            "clients:\n  - home\n  - { name: gw, target: 10.0.0.12:3000, weight: 2 }\n"
        );
    }

    #[test]
    fn an_email_is_written_plain_and_a_leading_at_sign_is_quoted() {
        assert_eq!(scalar_str("ops@example.com"), "ops@example.com");
        assert_eq!(scalar_str("@home"), "\"@home\"");
    }
    use crate::config::tree;

    #[test]
    fn yaml_round_trips_through_parser() {
        let v = serde_json::json!({
            "listen": "0.0.0.0:2333",
            "clients": {"home": {"secret": "+ab/c=="}},
            "services": {"web": {"bind": 8080, "client": "home", "target": "127.0.0.1:3000"}},
            "allow": ["127.0.0.0/8", "[::1/128]:22"],
            "empty": [],
            "flag": true,
            "num_str": "123",
            "env": "${HOME_SECRET}",
        });
        let text = render(&v, Format::Yaml);
        let parsed = tree::parse(&text).unwrap().to_json();
        assert_eq!(parsed, v, "{text}");
    }
}
