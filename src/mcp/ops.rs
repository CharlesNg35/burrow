use burrow_admin_proto::ops::Op;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::Value;

const EDITOR_ONLY: &str = "replace_text";

pub const TEXT_OP: &str = "replace_text is for burrow admin's text editor; use the other ops";

fn editor_only(branch: &Value) -> bool {
    branch.pointer("/properties/op/const") == Some(&Value::from(EDITOR_ONLY))
}

fn drop_editor_ops(schema: Option<&mut serde_json::Map<String, Value>>) {
    if let Some(Value::Array(branches)) = schema.and_then(|s| s.get_mut("oneOf")) {
        branches.retain(|b| !editor_only(b));
    }
}

pub fn ops_schema(g: &mut SchemaGenerator) -> Schema {
    let mut op = g.subschema_for::<Op>();
    match g.definitions_mut().get_mut(&Op::schema_name().to_string()) {
        Some(def) => drop_editor_ops(def.as_object_mut()),
        None => drop_editor_ops(op.as_object_mut()),
    }
    schemars::json_schema!({ "type": "array", "items": op })
}

pub fn editor_op(ops: &[Op]) -> bool {
    ops.iter().any(|o| matches!(o, Op::ReplaceText { .. }))
}
