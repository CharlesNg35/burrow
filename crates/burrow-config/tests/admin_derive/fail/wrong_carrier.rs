use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Service {
    #[admin(label = "Limit", ty = rate)]
    pub bandwidth: Option<u32>,
}

fn main() {}
