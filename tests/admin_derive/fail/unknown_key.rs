use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Service {
    #[admin(label = "On", lable = "Typo")]
    pub on: bool,
    #[admin(label = "Shade", ty = colour)]
    pub shade: String,
}

fn main() {}
