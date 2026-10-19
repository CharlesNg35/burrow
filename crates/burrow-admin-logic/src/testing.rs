use burrow_admin_proto::schema::Schema;
use burrow_admin_proto::serde_json::{self, Value, json};

pub fn schema() -> Schema {
    serde_json::from_str(include_str!(
        "../../burrow-admin-proto/tests/fixtures/v0.1/schema.json"
    ))
    .expect("the frozen schema parses")
}

pub fn secret(fp: &str) -> Value {
    json!({"$secret": {"fp": fp, "src": "literal"}})
}

pub fn doc() -> Value {
    json!({
        "listen": 7835,
        "clients": {"home": {"secret": secret("aaaa0001")}, "office": {"secret": secret("aaaa0002")}},
        "visitors": {"laptop": {"secret": secret("bbbb0001"), "reach": {"home": ["10.0.0.0/8"]}}},
        "services": {
            "web": {"client": "home", "bind": 7840, "target": 3000, "visitors": ["laptop"]},
            "both": {"clients": ["home", "office"], "bind": 7841, "target": 3001},
            "dns": {"client": "office", "protocol": "udp", "bind": 7853, "target": 53}
        }
    })
}
