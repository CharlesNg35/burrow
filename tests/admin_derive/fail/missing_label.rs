use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Limits {
    #[admin(ty = integer(unit = "connections"))]
    pub max_visitors: u32,
}

#[derive(AdminSchema)]
pub enum Mode {
    #[admin(label = "Failover")]
    Failover,
    #[admin(help = "Spread connections")]
    Balance,
}

fn main() {}
