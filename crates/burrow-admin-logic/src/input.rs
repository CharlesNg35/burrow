use burrow_admin_proto::schema::{DurationUnit, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_syntax::duration::TimeUnit;
use burrow_syntax::rate::{RateUnit, compose, split};

pub fn to_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) if burrow_admin_proto::value::as_secret(v).is_some() => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn whole(text: &str) -> Option<Value> {
    text.parse::<i64>().ok().map(Value::from)
}

pub fn parse_input(sem: &SemType, text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(match sem {
        SemType::Integer(_) | SemType::Size(_) | SemType::Duration(_) | SemType::Port => {
            whole(t).unwrap_or_else(|| Value::String(t.to_string()))
        }
        SemType::IpPort(_) | SemType::Target(_) => {
            if t.bytes().all(|b| b.is_ascii_digit()) {
                whole(t).unwrap_or_else(|| Value::String(t.to_string()))
            } else {
                Value::String(t.to_string())
            }
        }
        SemType::Bool => match t {
            "true" | "yes" => Value::Bool(true),
            "false" | "no" => Value::Bool(false),
            other => Value::String(other.to_string()),
        },
        SemType::Country(_) if t.len() == 2 => Value::String(t.to_ascii_uppercase()),
        SemType::Optional(o) => return parse_input(&o.inner.sem, text),
        _ => Value::String(t.to_string()),
    })
}

pub fn time_unit(u: DurationUnit) -> TimeUnit {
    match u {
        DurationUnit::Ms => TimeUnit::Ms,
        DurationUnit::Secs | DurationUnit::Unknown => TimeUnit::Secs,
    }
}

pub fn rate_parts(v: Option<&Value>) -> (String, RateUnit) {
    match v.and_then(Value::as_str).and_then(split) {
        Some((n, u)) => (n, u),
        None => (to_text(v), RateUnit::Mbit),
    }
}

pub fn rate_value(number: &str, unit: RateUnit) -> Option<Value> {
    let n = number.trim();
    (!n.is_empty()).then(|| Value::String(compose(n, unit)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::schema::{IntegerC, IpPortC, TargetC, TextC};
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn typed_text_becomes_the_stored_shape() {
        let int = SemType::Integer(IntegerC::default());
        assert_eq!(parse_input(&int, " 42 "), Some(json!(42)));
        assert_eq!(parse_input(&int, "4x"), Some(json!("4x")));
        assert_eq!(parse_input(&int, "  "), None);
        let bind = SemType::IpPort(IpPortC::default());
        assert_eq!(parse_input(&bind, "7840"), Some(json!(7840)));
        assert_eq!(
            parse_input(&bind, "127.0.0.1:7840"),
            Some(json!("127.0.0.1:7840"))
        );
        let target = SemType::Target(TargetC::default());
        assert_eq!(parse_input(&target, "3000-3002"), Some(json!("3000-3002")));
        assert_eq!(
            parse_input(&SemType::Text(TextC::default()), "x"),
            Some(json!("x"))
        );
        assert_eq!(to_text(Some(&json!(7840))), "7840");
        assert_eq!(to_text(None), "");
        let hidden = json!({"$secret": {"fp": null, "src": "literal"}});
        assert_eq!(
            to_text(Some(&hidden)),
            "",
            "a hidden value never fills a text box"
        );
    }

    #[test]
    fn rates_split_and_compose() {
        let (n, u) = rate_parts(Some(&json!("40mbit")));
        assert_eq!((n.as_str(), u), ("40", RateUnit::Mbit));
        assert_eq!(rate_value("5", RateUnit::MB), Some(json!("5MB")));
        assert_eq!(rate_value(" ", RateUnit::MB), None);
    }
}
