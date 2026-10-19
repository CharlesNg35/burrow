use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub enum Mode {
    #[admin(label = "Failover")]
    Failover,
    #[admin(label = "Balance")]
    Balance,
    Random,
}

fn main() {}
