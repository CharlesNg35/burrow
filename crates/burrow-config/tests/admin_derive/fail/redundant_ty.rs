use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Limits {
    #[admin(ty = integer)]
    pub max_visitors: u32,
}

fn main() {}
