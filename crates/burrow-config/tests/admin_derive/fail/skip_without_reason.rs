use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Entry {
    #[admin(skip)]
    pub installer_note: String,
    #[admin(skip = "")]
    pub other: String,
}

fn main() {}
