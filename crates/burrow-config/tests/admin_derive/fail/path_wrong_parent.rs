use burrow_config::{Pool, ServerConfig};

fn main() {
    let _ = ServerConfig::LIMITS.then(Pool::MAX_IDLE);
}
