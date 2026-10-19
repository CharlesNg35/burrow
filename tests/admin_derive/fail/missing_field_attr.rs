use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Limits {
    #[admin(label = "Max handshakes", ty = integer(unit = "connections"))]
    pub max_handshakes: u32,
    pub max_visitors: u32,
}

fn main() {}
