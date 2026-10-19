use burrow::config::admin_schema::AdminSchema;

#[derive(AdminSchema)]
pub struct Service {
    #[admin(ty = ref_list(client))]
    pub clients: Vec<String>,
    #[admin(visible_if(len_ge(clientz, 2)))]
    pub balance: bool,
}

fn main() {}
