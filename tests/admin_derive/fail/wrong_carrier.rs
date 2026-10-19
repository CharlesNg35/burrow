use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Service {
    #[admin(ty = rate)]
    pub bandwidth: Option<u32>,
}

fn main() {}
