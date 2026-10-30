use burrow_admin_proto::serde_json::{Map, Value};
use burrow_config::{Mode, ServiceEntry, WeightedClient, ref_name};
use burrow_ui_kit::TextInput;
use leptos::prelude::*;

use crate::domain::{FieldCtx, set_checked};
use crate::store::store;

fn weight_text(entry: &Value) -> String {
    match WeightedClient::WEIGHT.get(entry) {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => "1".into(),
    }
}

fn with_weight(list: &[Value], name: &str, text: &str) -> Vec<Value> {
    list.iter()
        .map(|entry| {
            if ref_name(entry) != Some(name) {
                return entry.clone();
            }
            let weight = match text.trim().parse::<i64>() {
                Ok(1) => return Value::String(name.to_string()),
                Ok(w) => Value::from(w),
                Err(_) => Value::String(text.to_string()),
            };
            let mut pair = Value::Object(Map::new());
            WeightedClient::NAME.set(&mut pair, Value::String(name.to_string()));
            WeightedClient::WEIGHT.set(&mut pair, weight);
            pair
        })
        .collect()
}

fn listed(path: &[String]) -> Vec<Value> {
    store()
        .edit
        .value(path)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn row(fc: FieldCtx, name: String) -> AnyView {
    let input_id = format!("{}-weight-{name}", fc.id);
    let (path, p2) = (fc.path.clone(), fc.path.clone());
    let n2 = name.clone();
    let text = Signal::derive(move || {
        listed(&p2)
            .iter()
            .find(|x| ref_name(x) == Some(n2.as_str()))
            .map(weight_text)
            .unwrap_or_else(|| "1".into())
    });
    let n3 = name.clone();
    let on_input = Callback::new(move |t: String| {
        let list = with_weight(&listed(&path), &n3, &t);
        set_checked(&fc, &fc.node, Some(Value::Array(list)));
    });
    view! {
        <div class="flex items-center gap-2" data-weight-of=name.clone()>
            <label for=input_id.clone() class="w-40 truncate font-mono text-sm">{name.clone()}</label>
            <div class="w-[160px]">
                <TextInput id=input_id value=text on_input=on_input numeric=true suffix="shares" label=format!("Weight of {name}")/>
            </div>
        </div>
    }
    .into_any()
}

pub fn weights(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let mode_path = {
        let mut p = fc.path.clone();
        p.pop();
        p.extend(ServiceEntry::MODE.rel());
        p
    };
    let path = fc.path.clone();
    let names = Signal::derive(move || {
        let balance = e
            .value(&mode_path)
            .and_then(|v| v.as_str().and_then(Mode::from_wire))
            == Some(Mode::Balance);
        let list = e.value(&path);
        let weighted = list
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| WeightedClient::WEIGHT.get(x).is_some()));
        let names = ServiceEntry::listed_names(list.as_ref());
        if (balance || weighted) && names.len() > 1 {
            names
        } else {
            Vec::new()
        }
    });
    view! {
        <Show when=move || !names.with(Vec::is_empty)>
            <div class="mt-2 space-y-1.5" data-weights="">
                <p class="text-xs text-text-muted">"Weight: a client with weight 3 gets three times the new connections of one with weight 1."</p>
                <For each=move || names.get() key=String::clone children={
                    let fc = fc.clone();
                    move |name| row(fc.clone(), name)
                }/>
            </div>
        </Show>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_weight_of_1_writes_the_plain_name_and_others_write_the_pair() {
        let list = vec![json!("a"), json!({"name": "b", "weight": 2})];
        assert_eq!(
            with_weight(&list, "a", "3"),
            vec![
                json!({"name": "a", "weight": 3}),
                json!({"name": "b", "weight": 2})
            ]
        );
        assert_eq!(with_weight(&list, "b", "1"), vec![json!("a"), json!("b")]);
        assert_eq!(
            with_weight(&list, "b", "x")[1],
            json!({"name": "b", "weight": "x"})
        );
        assert_eq!(weight_text(&list[0]), "1");
        assert_eq!(weight_text(&list[1]), "2");
    }
}
