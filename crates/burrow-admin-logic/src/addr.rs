use burrow_admin_proto::serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parts {
    pub host: String,
    pub from: String,
    pub to: String,
    pub unix: Option<String>,
}

fn port_part(p: &str) -> (String, String) {
    match p.split_once('-') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (p.trim().to_string(), String::new()),
    }
}

pub fn split_text(text: &str) -> Parts {
    let t = text.trim();
    if let Some(path) = t.strip_prefix("unix:") {
        return Parts {
            unix: Some(path.to_string()),
            ..Parts::default()
        };
    }
    if let Some(rest) = t.strip_prefix('[')
        && let Some((h, tail)) = rest.split_once(']')
    {
        let (from, to) = port_part(tail.trim_start_matches(':'));
        return Parts {
            host: h.to_string(),
            from,
            to,
            unix: None,
        };
    }
    match t.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => {
            let (from, to) = port_part(p);
            Parts {
                host: h.to_string(),
                from,
                to,
                unix: None,
            }
        }
        Some(_) => Parts {
            host: t.to_string(),
            ..Parts::default()
        },
        None if t.chars().next().is_some_and(|c| c.is_ascii_digit()) && !t.contains('.') => {
            let (from, to) = port_part(t);
            Parts {
                from,
                to,
                ..Parts::default()
            }
        }
        None => Parts {
            host: t.to_string(),
            ..Parts::default()
        },
    }
}

pub fn split(v: Option<&Value>) -> Parts {
    match v {
        Some(Value::Number(n)) => Parts {
            from: n.to_string(),
            ..Parts::default()
        },
        Some(Value::String(s)) => split_text(s),
        _ => Parts::default(),
    }
}

pub fn join(p: &Parts) -> Option<Value> {
    if let Some(path) = &p.unix {
        return (!path.trim().is_empty()).then(|| Value::String(format!("unix:{}", path.trim())));
    }
    let (host, from, to) = (p.host.trim(), p.from.trim(), p.to.trim());
    if host.is_empty() && from.is_empty() {
        return None;
    }
    let port = if to.is_empty() {
        from.to_string()
    } else {
        format!("{from}-{to}")
    };
    if host.is_empty() {
        return Some(match port.parse::<u64>() {
            Ok(n) => Value::from(n),
            Err(_) => Value::String(port),
        });
    }
    let host = burrow_syntax::address::url_host(host).into_owned();
    Some(Value::String(if port.is_empty() {
        host
    } else {
        format!("{host}:{port}")
    }))
}

pub fn count(p: &Parts) -> Option<u32> {
    if p.unix.is_some() {
        return Some(1);
    }
    let a: u32 = p.from.trim().parse().ok()?;
    if p.to.trim().is_empty() {
        return Some(1);
    }
    let b: u32 = p.to.trim().parse().ok()?;
    (b >= a).then(|| b - a + 1)
}

pub fn looks_whole(text: &str) -> bool {
    let t = text.trim();
    t.contains(':') || t.contains('-') || t.contains('.') || t.contains('[')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub scheme: &'static str,
    pub rest: String,
}

pub fn split_server(v: Option<&Value>) -> Server {
    let t = v.and_then(Value::as_str).unwrap_or_default();
    for scheme in ["wss", "ws"] {
        if let Some(rest) = t.strip_prefix(&format!("{scheme}://")) {
            return Server {
                scheme,
                rest: rest.to_string(),
            };
        }
    }
    Server {
        scheme: "name",
        rest: t.to_string(),
    }
}

pub fn join_server(s: &Server) -> Option<Value> {
    let rest = s.rest.trim();
    if rest.is_empty() {
        return None;
    }
    Some(Value::String(match s.scheme {
        "name" => rest.to_string(),
        scheme => format!("{scheme}://{rest}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn binds_split_and_join_back_to_the_same_value() {
        for v in [
            json!("0.0.0.0:30000-30010"),
            json!("127.0.0.1:3000"),
            json!("[::]:443"),
            json!(2222),
            json!("30000-30010"),
            json!("10.0.4.20:30000-30010"),
            json!("unix:/var/run/docker.sock"),
            json!("db.internal:5432"),
        ] {
            assert_eq!(join(&split(Some(&v))), Some(v.clone()), "{v}");
        }
        let p = split(Some(&json!("0.0.0.0:30000-30010")));
        assert_eq!(
            (p.host.as_str(), p.from.as_str(), p.to.as_str()),
            ("0.0.0.0", "30000", "30010")
        );
        assert_eq!(count(&p), Some(11));
        assert_eq!(count(&split(Some(&json!(22)))), Some(1));
        assert_eq!(count(&split_text("10-5")), None);
        assert_eq!(split(Some(&json!("[::1]:8443"))).host, "::1");
        assert_eq!(join(&Parts::default()), None);
        assert_eq!(
            join(&Parts {
                host: "::".into(),
                from: "443".into(),
                ..Parts::default()
            }),
            Some(json!("[::]:443"))
        );
    }

    #[test]
    fn a_pasted_address_is_recognised_while_a_port_is_not() {
        assert!(looks_whole("127.0.0.1:3000"));
        assert!(looks_whole("30000-30010"));
        assert!(!looks_whole("3000"));
        assert_eq!(split_text("example.com").host, "example.com");
    }

    #[test]
    fn server_addresses_keep_their_scheme() {
        for v in [
            "tunnel.example.net",
            "tunnel.example.net:7835",
            "wss://tunnel.example.net/burrow",
            "ws://10.0.0.1:80/b",
        ] {
            let s = split_server(Some(&json!(v)));
            assert_eq!(join_server(&s), Some(json!(v)));
        }
        assert_eq!(split_server(Some(&json!("wss://x/y"))).scheme, "wss");
        assert_eq!(
            join_server(&Server {
                scheme: "ws",
                rest: " ".into()
            }),
            None
        );
    }
}
