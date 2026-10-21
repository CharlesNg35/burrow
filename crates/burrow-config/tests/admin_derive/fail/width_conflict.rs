use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Entry {
    #[admin(wide, narrow)]
    pub level: u32,
}

fn main() {}
