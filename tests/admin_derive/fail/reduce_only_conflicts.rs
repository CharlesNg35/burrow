use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Entry {
    #[admin(locked, reduce_only)]
    pub level: u32,
    #[admin(readonly = "fixed", reduce_only)]
    pub other: u32,
    #[admin(reduce_only = "yes")]
    pub third: u32,
}

fn main() {}
