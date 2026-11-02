use burrow_admin_proto::schema::{
    DurationC, DurationUnit, IpC, IpPortC, NameC, Node, ObjectC, PortRangeC, RefListC, SemType,
    TargetC, UnionC,
};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::is_special;
use burrow_syntax::duration::{TimeUnit, check_range};
use burrow_syntax::{hostport, ip, name, ports, rate, rule, target};

use crate::changeset::as_new_secret;
use crate::fallback::{Render, for_node};

const GROUP: &str = "This needs several settings, not a single value";

pub fn check(node: &Node, v: &Value) -> Result<(), String> {
    if v.is_null() {
        return if node.required && node.default.is_none() {
            Err(crate::wording::required(&node.label))
        } else {
            Ok(())
        };
    }
    if is_special(v) || as_new_secret(v).is_some() || crate::changeset::as_new_password(v).is_some()
    {
        return Ok(());
    }
    match for_node(node, Some(v)) {
        Render::Known(sem) | Render::Fallback(sem) => check_sem(sem, v),
        Render::ServerChecked | Render::Raw => Ok(()),
    }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn want_text(v: &Value) -> Result<String, String> {
    text(v).ok_or_else(|| "expected text".to_string())
}

fn want_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| "expected a whole number".to_string()),
        Value::String(s) => Err(format!("\"{s}\" is not a whole number")),
        _ => Err("expected a whole number".into()),
    }
}

fn want_str(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| "expected text".to_string())
}

fn port(v: &Value) -> Result<u16, String> {
    match v {
        Value::Number(_) => {
            let n = want_int(v)?;
            u16::try_from(n)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("{n} is not a port (1 to 65535)"))
        }
        _ => ports::parse_port(&want_text(v)?),
    }
}

fn check_sem(sem: &SemType, v: &Value) -> Result<(), String> {
    match sem {
        SemType::Text(c) => {
            let s = want_text(v)?;
            match c.max_len {
                Some(m) if s.chars().count() > m as usize => Err(format!("at most {m} characters")),
                _ => Ok(()),
            }
        }
        SemType::Integer(c) => {
            let n = want_int(v)?;
            match (c.min, c.max) {
                (Some(a), _) if n < a => Err(format!("at least {a}")),
                (_, Some(b)) if n > b => Err(format!("at most {b}")),
                _ => Ok(()),
            }
        }
        SemType::Bool => v
            .is_boolean()
            .then_some(())
            .ok_or_else(|| "Pick Yes or No".to_string()),
        SemType::Enum(c) => {
            let s = want_text(v)?;
            if c.options.iter().any(|o| o.value == s) {
                Ok(())
            } else {
                let names: Vec<String> = c
                    .options
                    .iter()
                    .map(|o| if o.label.is_empty() { o.value.clone() } else { o.label.clone() })
                    .collect();
                Err(format!("Pick one: {}", names.join(", ")))
            }
        }
        SemType::Ip(c) => check_ip(c, v),
        SemType::Port => match v {
            Value::String(s) => ports::parse_port(s).map(|_| ()),
            _ => port(v).map(|_| ()),
        },
        SemType::IpPort(c) => check_ip_port(c, v),
        SemType::PortRange(c) => check_port_range(c, v),
        SemType::Target(c) => check_target(c, v),
        SemType::Host => hostport::check_host(&want_text(v)?),
        SemType::HostName(_) => {
            burrow_syntax::web::host_name(&want_text(v)?, burrow_syntax::web::SingleLabel::Refused)
                .map(|_| ())
        }
        SemType::ServerAddress => burrow_syntax::endpoint::check_public_address(&want_text(v)?),
        SemType::Cidr => want_text(v)?
            .parse::<burrow_syntax::cidr::Cidr>()
            .map(|_| ())
            .map_err(|e| e.describe()),
        SemType::CidrRule => want_text(v)?
            .parse::<rule::Rule>()
            .map(|_| ())
            .map_err(|e| e.describe()),
        SemType::Country(_) => burrow_syntax::country::country(&want_text(v)?).map(|_| ()),
        SemType::Duration(c) => check_duration(c, v),
        SemType::Size(_) => {
            let n = want_int(v)?;
            (n >= 0)
                .then_some(())
                .ok_or_else(|| "a size cannot be negative".to_string())
        }
        SemType::Rate => rate::parse_rate(&want_text(v)?).map(|_| ()),
        SemType::Name(c) => check_name(c, v),
        SemType::Ref(_) => {
            let s = want_str(v)?;
            (!s.trim().is_empty())
                .then_some(())
                .ok_or_else(|| "pick one".to_string())
        }
        SemType::RefList(c) => check_ref_list(c, v),
        SemType::RefEntries(c) => crate::entries::check(c, v),
        SemType::Path(_) => {
            let s = want_text(v)?;
            (!s.trim().is_empty())
                .then_some(())
                .ok_or_else(|| "a path is required".to_string())
        }
        SemType::Upload(c) => {
            let s = want_str(v)?;
            (burrow_syntax::upload::kind_of(s.trim()).is_some_and(|k| k.prefix() == c.kind))
                .then_some(())
                .ok_or_else(|| "upload the file again".to_string())
        }
        SemType::PasswordHash(_) => want_text(v).map(|_| ()),
        SemType::UrlPath => {
            let s = want_text(v)?;
            s.starts_with('/')
                .then_some(())
                .ok_or_else(|| "starts with /".to_string())
        }
        SemType::List(c) => match v {
            Value::Array(a) => match c.max_items {
                Some(m) if a.len() > m as usize => Err(format!("at most {m} items")),
                _ => a.iter().try_for_each(|x| check(&c.item, x)),
            },
            _ => Err("expected a list".into()),
        },
        SemType::Optional(o) => check(&o.inner, v),
        SemType::Union(u) => check_union(u, v),
        SemType::Object(o) => check_object(o, v),
        SemType::Map(c) => match v {
            Value::Object(m) => m.values().try_for_each(|x| check(&c.value, x)),
            _ => Err(GROUP.into()),
        },
        SemType::RefMap(c) => match v {
            Value::Object(m) => m.values().try_for_each(|x| check(&c.value, x)),
            _ => Err(GROUP.into()),
        },
        SemType::SecretRef(c) if c.reference => match v {
            Value::String(s) if !burrow_syntax::env::is_reference(s) => Err(
                "write an environment variable like ${CF_TOKEN} or a file like file:/etc/burrow/token, never the secret itself".into(),
            ),
            _ => Ok(()),
        },
        SemType::SecretRef(_) | SemType::Unknown(_) => Ok(()),
    }
}

fn check_ip(c: &IpC, v: &Value) -> Result<(), String> {
    let a = ip::parse_ip(&want_text(v)?)?;
    if a.is_ipv4() && !c.v4 && c.v6 {
        return Err("an IPv6 address is needed here".into());
    }
    if a.is_ipv6() && !c.v6 && c.v4 {
        return Err("an IPv4 address is needed here".into());
    }
    if a.is_unspecified() && !c.unspecified_ok {
        return Err("0.0.0.0 and :: are not allowed here".into());
    }
    Ok(())
}

fn check_ip_port(c: &IpPortC, v: &Value) -> Result<(), String> {
    if v.is_number() {
        return port(v).map(|_| ());
    }
    let max = if c.ranges {
        c.max_range.unwrap_or(ports::MAX_RANGE)
    } else {
        1
    };
    target::parse_bind(&want_text(v)?, [0, 0, 0, 0], max).map(|_| ())
}

fn check_port_range(c: &PortRangeC, v: &Value) -> Result<(), String> {
    let (a, b) = ports::parse_range(&want_text(v)?)?;
    let count = u32::from(b) - u32::from(a) + 1;
    match c.max {
        Some(m) if count > u32::from(m) => Err(format!("at most {m} ports")),
        _ => Ok(()),
    }
}

fn check_target(c: &TargetC, v: &Value) -> Result<(), String> {
    if v.is_number() {
        return port(v).map(|_| ());
    }
    let t = target::parse_target(&want_text(v)?)?;
    match t {
        target::TargetValue::Unix(_) if !c.unix => Err("a Unix socket is not allowed here".into()),
        t if t.count() > 1 && !c.ranges => Err("a single port is needed here".into()),
        _ => Ok(()),
    }
}

fn check_duration(c: &DurationC, v: &Value) -> Result<(), String> {
    let n = want_int(v)?;
    let n = u64::try_from(n).map_err(|_| "a duration cannot be negative".to_string())?;
    let unit = match c.unit {
        DurationUnit::Ms => TimeUnit::Ms,
        _ => TimeUnit::Secs,
    };
    check_range(n, c.min, c.max, unit)
}

fn check_name(c: &NameC, v: &Value) -> Result<(), String> {
    let s = want_text(v)?;
    name::check_name(&s)?;
    if s.chars().count() > c.max_len as usize {
        return Err(format!("at most {} characters", c.max_len));
    }
    Ok(())
}

fn check_ref_list(c: &RefListC, v: &Value) -> Result<(), String> {
    let items = match v {
        Value::Array(a) => a.clone(),
        _ => return Err("expected a list of names".into()),
    };
    let names: Vec<String> = items
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    if names.len() != items.len() {
        return Err("expected a list of names".into());
    }
    if let Some(m) = c.max
        && names.len() > m as usize
    {
        return Err(format!("at most {m}"));
    }
    if c.unique
        && let Some(d) = names
            .iter()
            .enumerate()
            .find(|(i, n)| names[..*i].contains(n))
    {
        return Err(format!("\"{}\" is listed twice", d.1));
    }
    Ok(())
}

fn shape_fits(b: &Node, v: &Value) -> bool {
    match (&crate::tree::inner(b).sem, v) {
        (SemType::Object(_), Value::Object(_)) | (SemType::List(_), Value::Array(_)) => true,
        (SemType::Object(_) | SemType::List(_), _) => false,
        (_, Value::Object(_) | Value::Array(_)) => false,
        _ => true,
    }
}

fn check_union(u: &UnionC, v: &Value) -> Result<(), String> {
    if u.branches.is_empty() || u.branches.iter().any(|b| check(b, v).is_ok()) {
        return Ok(());
    }
    let fitting = u.branches.iter().find(|b| shape_fits(b, v));
    fitting
        .into_iter()
        .chain(u.branches.iter())
        .map(|b| check(b, v))
        .find_map(Result::err)
        .map_or(Ok(()), Err)
}

fn check_object(o: &ObjectC, v: &Value) -> Result<(), String> {
    let Value::Object(m) = v else {
        return Err(GROUP.into());
    };
    m.iter()
        .try_for_each(|(k, x)| match o.fields.iter().find(|f| f.key == *k) {
            Some(f) => check(&f.node, x).map_err(|e| {
                let name = if f.node.label.is_empty() {
                    k.as_str()
                } else {
                    f.node.label.as_str()
                };
                format!("{name}: {e}")
            }),
            None => Err(format!(
                "{k} is not a setting burrow knows here; remove it in server.yaml"
            )),
        })
}

pub fn check_in(node: &Node, v: &Value, doc: &Value) -> Result<(), String> {
    check(node, v)?;
    match (&crate::tree::inner(node).sem, v) {
        (SemType::ServerAddress, Value::String(s)) => public_address_in(s, doc),
        _ => Ok(()),
    }
}

enum Bound {
    Missing,
    Unknown,
    At(std::net::SocketAddr),
}

fn bind_at(v: Option<&Value>) -> Bound {
    let text = match v {
        None | Some(Value::Null) => return Bound::Missing,
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => match burrow_admin_proto::value::as_env(other).and_then(|e| e.resolved) {
            Some(r) => r,
            None => return Bound::Unknown,
        },
    };
    match target::parse_bind(&text, [0, 0, 0, 0], 1) {
        Ok(b) => Bound::At(std::net::SocketAddr::new(b.ip, b.first)),
        Err(_) => Bound::Unknown,
    }
}

fn public_address_in(given: &str, doc: &Value) -> Result<(), String> {
    use burrow_config::{ServerConfig, WebSocketConfig};
    let listen = match bind_at(ServerConfig::LISTEN.get(doc)) {
        Bound::Unknown => return Ok(()),
        Bound::Missing => None,
        Bound::At(a) => Some(a),
    };
    let ws = ServerConfig::WEBSOCKET.get(doc).filter(|w| w.is_object());
    let path = ws
        .and_then(|w| WebSocketConfig::PATH.str(w))
        .map(str::to_string)
        .unwrap_or_else(burrow_config::default_ws_path);
    let shape = match ws.map(|w| bind_at(WebSocketConfig::LISTEN.get(w))) {
        Some(Bound::Unknown) => return Ok(()),
        Some(Bound::At(listen)) => Some(burrow_syntax::address::WsShape {
            listen,
            path: &path,
            tls: ws
                .and_then(|w| WebSocketConfig::TLS.get(w))
                .is_some_and(|t| !t.is_null()),
        }),
        _ => None,
    };
    if listen.is_none() && shape.is_none() {
        return Ok(());
    }
    burrow_syntax::address::client_address(
        given,
        listen,
        shape,
        ServerConfig::CLIENTS.key(),
        &mut Vec::new(),
    )
    .map(|_| ())
    .map_err(|e| e.message)
}

pub fn refs_exist(node: &Node, v: &Value, names: &[String]) -> Result<(), String> {
    let reserved = node.sem.reserved();
    let missing = |n: &str| {
        (!names.iter().chain(reserved).any(|x| x == n)).then(|| format!("\"{n}\" does not exist"))
    };
    match (&node.sem, v) {
        (SemType::Ref(_), Value::String(s)) => missing(s).map_or(Ok(()), Err),
        (SemType::RefList(_), Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .find_map(missing)
            .map_or(Ok(()), Err),
        (SemType::RefList(_), Value::String(s)) => missing(s).map_or(Ok(()), Err),
        (SemType::RefEntries(_), Value::Array(a)) => a
            .iter()
            .filter_map(burrow_config::ref_name)
            .find_map(missing)
            .map_or(Ok(()), Err),
        (SemType::RefEntries(_), Value::String(s)) => missing(s).map_or(Ok(()), Err),
        (SemType::Optional(o), _) => refs_exist(&o.inner, v, names),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::schema::{
        DurationC, EnumC, EnumOption, IntegerC, IpPortC, NameC, RefListC, TargetC, UnknownType,
    };
    use burrow_admin_proto::serde_json::json;

    fn node(sem: SemType) -> Node {
        Node {
            label: "Field".into(),
            ..Node::new(sem)
        }
    }

    fn ok(sem: SemType, v: Value) {
        assert_eq!(check(&node(sem.clone()), &v), Ok(()), "{sem:?} {v}");
    }

    fn bad(sem: SemType, v: Value) -> String {
        check(&node(sem.clone()), &v).expect_err(&format!("{sem:?} accepted {v}"))
    }

    #[test]
    fn a_reference_secret_takes_a_variable_or_a_file_only() {
        use burrow_admin_proto::schema::SecretRefC;
        let node = Node::new(SemType::SecretRef(SecretRefC {
            max: 1,
            reference: true,
        }));
        assert!(check(&node, &json!("${CF_TOKEN}")).is_ok());
        assert!(check(&node, &json!("file:/etc/burrow/cf.token")).is_ok());
        let e = check(&node, &json!("pasted-token")).unwrap_err();
        assert!(e.contains("never the secret itself"), "{e}");
        let tunnel = Node::new(SemType::SecretRef(SecretRefC {
            max: 4,
            reference: false,
        }));
        assert!(check(&tunnel, &json!("anything")).is_ok());
    }

    #[test]
    fn ports_and_binds() {
        ok(SemType::Port, json!(7835));
        ok(SemType::Port, json!("7835"));
        bad(SemType::Port, json!(0));
        bad(SemType::Port, json!(70000));
        let bind = SemType::IpPort(IpPortC {
            ranges: true,
            max_range: Some(100),
            default_ip: None,
        });
        ok(bind.clone(), json!(7840));
        ok(bind.clone(), json!("127.0.0.1:7840"));
        ok(bind.clone(), json!("7840-7849"));
        assert!(bad(bind.clone(), json!("1000-2000")).contains("100"));
        bad(bind.clone(), json!("localhost:99999"));
        let single = SemType::IpPort(IpPortC::default());
        bad(single, json!("7840-7841"));
    }

    #[test]
    fn server_addresses_take_a_host_host_port_or_a_websocket_url() {
        for v in [
            "tunnel.example.net",
            "tunnel.example.net:7835",
            "203.0.113.7",
            "wss://tunnel.example.net/burrow",
            "ws://10.0.0.1:8080/b",
            "[2001:db8::1]:443",
        ] {
            ok(SemType::ServerAddress, json!(v));
        }
        bad(SemType::ServerAddress, json!("bad host"));
        bad(SemType::ServerAddress, json!("x:99999"));
        bad(SemType::ServerAddress, json!("wss://"));
        bad(SemType::ServerAddress, json!("https://x"));
    }

    #[test]
    fn a_union_names_the_problem_of_the_branch_the_value_looks_like() {
        let names = SemType::Union(burrow_admin_proto::schema::UnionC {
            branches: vec![
                Node::new(SemType::HostName(Default::default())),
                Node::new(SemType::List(burrow_admin_proto::schema::ListC {
                    item: Box::new(Node::new(SemType::HostName(Default::default()))),
                    max_items: None,
                })),
            ],
        });
        assert!(
            bad(
                names.clone(),
                json!(["ok.example.com", "https://x.example.com"])
            )
            .contains("the name alone")
        );
        assert!(bad(names, json!("*.com")).contains("too wide"));
    }

    #[test]
    fn host_names_take_names_wildcards_and_addresses_like_validate() {
        let h = || SemType::HostName(burrow_admin_proto::schema::HostNameC::default());
        for v in [
            "blog.example.com",
            "*.apps.example.com",
            "203.0.113.10",
            "2001:db8::1",
        ] {
            ok(h(), json!(v));
        }
        assert!(bad(h(), json!("https://blog.example.com")).contains("the name alone"));
        assert!(bad(h(), json!("*.com")).contains("too wide"));
        bad(h(), json!("nas"));
        bad(h(), json!(7));
    }

    #[test]
    fn targets() {
        let t = SemType::Target(TargetC {
            ranges: true,
            unix: true,
        });
        ok(t.clone(), json!(3000));
        ok(t.clone(), json!("10.0.0.5:22"));
        ok(t.clone(), json!("unix:/run/app.sock"));
        bad(t, json!(""));
        let plain = SemType::Target(TargetC::default());
        bad(plain.clone(), json!("unix:/run/app.sock"));
        bad(plain, json!("127.0.0.1:3000-3001"));
    }

    #[test]
    fn numbers_durations_rates_names() {
        let n = SemType::Integer(IntegerC {
            min: Some(1),
            max: Some(10),
            unit: None,
        });
        ok(n.clone(), json!(5));
        assert_eq!(bad(n.clone(), json!(0)), "at least 1");
        assert_eq!(bad(n.clone(), json!(11)), "at most 10");
        bad(n, json!("five"));
        let d = SemType::Duration(DurationC {
            unit: DurationUnit::Secs,
            min: Some(1),
            max: None,
        });
        ok(d.clone(), json!(30));
        bad(d.clone(), json!(0));
        bad(d, json!(-5));
        ok(SemType::Rate, json!("40mbit"));
        bad(SemType::Rate, json!("fast"));
        let name = SemType::Name(NameC {
            entity: burrow_admin_proto::schema::Entity::Service,
            max_len: 8,
        });
        ok(name.clone(), json!("web"));
        bad(name.clone(), json!("has space"));
        bad(name, json!("muchtoolongname"));
    }

    #[test]
    fn choices_cidrs_and_lists() {
        let e = SemType::Enum(EnumC {
            options: vec![
                EnumOption {
                    value: "tcp".into(),
                    label: "TCP".into(),
                    help: String::new(),
                    danger: false,
                },
                EnumOption {
                    value: "udp".into(),
                    label: "UDP".into(),
                    help: String::new(),
                    danger: false,
                },
            ],
        });
        ok(e.clone(), json!("udp"));
        assert_eq!(bad(e, json!("sctp")), "Pick one: TCP, UDP");
        ok(SemType::Cidr, json!("10.0.0.0/8"));
        bad(SemType::Cidr, json!("10.0.0.0/99"));
        ok(SemType::Cidr, json!("[2001:db8::1]"));
        ok(SemType::Cidr, json!("[2001:db8::/32]"));
        for text in [
            "10.0.0.0/8",
            "203.0.113.7",
            "2001:db8::/32",
            "[2001:db8::1]",
            "[2001:db8::/32]",
            "[2001:db8::]/32",
            "10.0.0.1/8",
            "10.0.0.0/8:22",
            "[2001:db8::1]:22",
            "::ffff:10.0.0.0/104",
            "home",
            "",
        ] {
            let server = text.parse::<burrow_syntax::cidr::Cidr>().is_ok();
            let ui = check_sem(&SemType::Cidr, &json!(text)).is_ok();
            assert_eq!(ui, server, "{text}");
        }
        ok(SemType::CidrRule, json!("10.0.0.0/8"));
        let refs = SemType::RefList(RefListC {
            entity: burrow_admin_proto::schema::Entity::Client,
            ordered: true,
            unique: true,
            max: None,
            reserved: vec!["self".into()],
        });
        ok(refs.clone(), json!(["home", "office"]));
        assert!(bad(refs.clone(), json!(["home", "home"])).contains("twice"));
        let n = node(refs);
        assert_eq!(
            refs_exist(&n, &json!(["home", "x"]), &["home".into()]),
            Err("\"x\" does not exist".into())
        );
        assert_eq!(
            refs_exist(&n, &json!(["home", "self"]), &["home".into()]),
            Ok(()),
            "a reserved name is always a valid reference"
        );
    }

    #[test]
    fn special_values_and_unknown_types_are_left_to_the_server() {
        ok(
            SemType::Port,
            json!({"$env": {"raw": "${P}", "vars": ["P"]}}),
        );
        ok(
            SemType::Port,
            json!({"$new_secret": {"handle": "h", "fp": "f"}}),
        );
        let future = SemType::Unknown(UnknownType {
            t: "rate_v2".into(),
            fallback: vec![SemType::Rate],
            raw: json!({"t": "rate_v2"}),
        });
        bad(future.clone(), json!("abc"));
        ok(future, json!("40mbit"));
        let opaque = SemType::Unknown(UnknownType {
            t: "geo".into(),
            fallback: vec![],
            raw: json!({"t": "geo"}),
        });
        ok(opaque, json!({"anything": 1}));
        let mut required = node(SemType::Port);
        required.required = true;
        assert_eq!(
            check(&required, &Value::Null),
            Err("Field is required".into())
        );
    }
}
