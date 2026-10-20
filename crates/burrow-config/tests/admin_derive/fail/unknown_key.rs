use burrow_config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Service {
    #[admin(lable = "Typo")]
    pub on: bool,
    #[admin(ty = colour)]
    pub shade: String,
}

fn main() {}
