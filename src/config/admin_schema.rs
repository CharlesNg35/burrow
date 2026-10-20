use burrow_admin_proto::schema::Schema;
use burrow_config::admin_schema::{AdminSchema, entities};

pub fn server_schema() -> Schema {
    let root = <burrow_config::ServerConfig as AdminSchema>::node();
    let mut schema = Schema {
        schema_version: burrow_admin_proto::SCHEMA_VERSION,
        hash: String::new(),
        root,
        entities: entities(),
    };
    schema.hash = schema_hash(&schema);
    schema
}

pub fn schema_hash(schema: &Schema) -> String {
    use blake2::Digest;
    let bytes = serde_json::to_vec(&Schema {
        hash: String::new(),
        ..schema.clone()
    })
    .unwrap_or_default();
    let digest = blake2::Blake2s256::digest(&bytes);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("b2s:{hex}")
}

#[cfg(test)]
mod tests;
