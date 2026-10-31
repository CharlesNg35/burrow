use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Limits {
    #[admin(label = "Maximum visitors")]
    pub max_visitors: u32,
}

#[derive(serde::Serialize, AdminSchema)]
#[serde(rename_all = "lowercase")]
pub enum Version {
    #[admin(label = "v1")]
    V1,
    #[admin(label = "v2 (binary)")]
    V2,
}

fn main() {}
