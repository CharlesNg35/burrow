use burrow_admin_proto::schema::{Node, RefEntriesC, SemType};
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_config::{ServiceClient, ref_name};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub name: String,
    pub target: Option<Value>,
    pub weight: Option<Value>,
}

impl Row {
    pub fn named(name: &str) -> Row {
        Row {
            name: name.to_string(),
            ..Row::default()
        }
    }

    pub fn to_value(&self) -> Value {
        if self.target.is_none() && self.weight.is_none() {
            return Value::String(self.name.clone());
        }
        let mut v = Value::Object(Map::new());
        ServiceClient::NAME.set(&mut v, Value::String(self.name.clone()));
        if let Some(t) = &self.target {
            ServiceClient::TARGET.set(&mut v, t.clone());
        }
        if let Some(w) = &self.weight {
            ServiceClient::WEIGHT.set(&mut v, w.clone());
        }
        v
    }

    pub fn same_entry(&self, other: &Row) -> bool {
        self.name == other.name && target_text(&self.target) == target_text(&other.target)
    }
}

fn target_text(t: &Option<Value>) -> Option<String> {
    t.as_ref().map(|t| match t {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    })
}

const ENTRY_SHAPE: &str = "Each entry needs a client name, and may have a target and a weight";

pub fn rows(v: Option<&Value>) -> Vec<Row> {
    let row = |x: &Value| {
        ref_name(x).map(|n| Row {
            name: n.to_string(),
            target: ServiceClient::TARGET.get(x).cloned(),
            weight: ServiceClient::WEIGHT.get(x).cloned(),
        })
    };
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(row).collect(),
        Some(s @ Value::String(_)) => row(s).into_iter().collect(),
        _ => Vec::new(),
    }
}

pub fn to_values(rows: &[Row]) -> Vec<Value> {
    rows.iter().map(Row::to_value).collect()
}

fn known_keys(v: &Value) -> bool {
    let known = [
        ServiceClient::NAME.key(),
        ServiceClient::TARGET.key(),
        ServiceClient::WEIGHT.key(),
    ];
    v.as_object()
        .is_some_and(|m| m.keys().all(|k| known.contains(&k.as_str())))
}

fn part(sem: SemType, v: &Value) -> Result<(), String> {
    crate::check::check(
        &Node {
            sem,
            ..Node::default()
        },
        v,
    )
}

pub fn check(c: &RefEntriesC, v: &Value) -> Result<(), String> {
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::String(_) => vec![v],
        _ => return Err("expected a list of clients".into()),
    };
    let mut seen: Vec<Row> = Vec::new();
    for item in items {
        if !item.is_string() && !known_keys(item) {
            return Err(ENTRY_SHAPE.into());
        }
        let Some(row) = rows(Some(&Value::Array(vec![item.clone()]))).pop() else {
            return Err(ENTRY_SHAPE.into());
        };
        let name = &row.name;
        if let Some(t) = &row.target {
            part(SemType::Target(c.target.clone()), t)
                .map_err(|e| format!("Target of {name}: {e}"))?;
        }
        if let Some(w) = &row.weight {
            part(SemType::Integer(c.weight.clone()), w)
                .map_err(|e| format!("Weight of {name}: {e}"))?;
        }
        if seen.iter().any(|s| s.same_entry(&row)) {
            return Err(match row.target {
                Some(_) => format!("\"{name}\" is listed twice with the same target"),
                None => format!("\"{name}\" is listed twice"),
            });
        }
        seen.push(row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::schema::Entity;
    use burrow_admin_proto::serde_json::json;

    fn c() -> RefEntriesC {
        RefEntriesC::new(Entity::Client, Vec::new())
    }

    #[test]
    fn entries_take_a_target_of_their_own_and_a_weight_from_1_to_1000() {
        let ok = json!(["a", {"name": "gw", "target": "10.0.0.12:3000"}, {"name": "gw", "target": 3001, "weight": 3}]);
        assert_eq!(check(&c(), &ok), Ok(()));
        let e = check(&c(), &json!([{"name": "b", "weight": 0}])).unwrap_err();
        assert!(e.contains("Weight of b"), "{e}");
        let e = check(&c(), &json!([{"name": "b", "target": "nope:"}])).unwrap_err();
        assert!(e.contains("Target of b"), "{e}");
        let e = check(&c(), &json!([{"name": "b", "wieght": 2}])).unwrap_err();
        assert_eq!(e, ENTRY_SHAPE);
    }

    #[test]
    fn a_client_repeats_only_with_another_target() {
        let e = check(&c(), &json!(["a", {"name": "a", "weight": 2}])).unwrap_err();
        assert!(e.contains("listed twice"), "{e}");
        let e = check(
            &c(),
            &json!([{"name": "a", "target": 80}, {"name": "a", "target": 80}]),
        )
        .unwrap_err();
        assert!(e.contains("same target"), "{e}");
        assert_eq!(
            check(
                &c(),
                &json!([{"name": "a", "target": 80}, {"name": "a", "target": 81}])
            ),
            Ok(())
        );
    }

    #[test]
    fn a_row_without_target_or_weight_is_written_as_a_name() {
        let v = json!(["a", {"name": "gw", "target": 3001, "weight": 2}]);
        let rows = rows(Some(&v));
        assert_eq!(rows[0], Row::named("a"));
        assert_eq!(to_values(&rows), v.as_array().unwrap().clone());
        assert_eq!(
            Row {
                weight: None,
                ..rows[1].clone()
            }
            .to_value(),
            json!({"name": "gw", "target": 3001})
        );
    }
}
