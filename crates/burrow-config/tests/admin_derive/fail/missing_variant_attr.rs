use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub enum Mode {
    #[admin]
    Failover,
    #[admin]
    Balance,
    Random,
}

fn main() {}
