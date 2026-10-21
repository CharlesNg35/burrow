use burrow_config::admin_schema::AdminSchema;

mod wire {
    #[derive(Clone, Copy)]
    pub enum Level {
        Read,
        Write,
        Owner,
    }
}

#[derive(serde::Serialize, AdminSchema)]
#[admin(remote = wire::Level)]
#[serde(rename_all = "lowercase")]
enum LevelSchema {
    #[admin]
    Read,
    #[admin]
    Write,
}

fn main() {}
