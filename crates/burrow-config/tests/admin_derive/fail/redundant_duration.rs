use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Limits {
    #[admin(ty = duration(secs))]
    pub timeout_secs: u32,
}

fn main() {}
