#[path = "init/client_config.rs"]
mod client_config;
mod common;
#[path = "init/outputs.rs"]
mod outputs;
#[path = "init/server_init.rs"]
mod server_init;
#[path = "init/visitor_config.rs"]
mod visitor_config;

use std::path::Path;

use burrow::config::env::ProcessEnv;
use burrow::config::{Loaded, load_file};

fn load(path: &Path) -> Loaded {
    let o = load_file(path, None, &ProcessEnv).unwrap();
    assert!(
        !o.report.has_errors(),
        "{}",
        o.report
            .render_text(burrow::base::style::Style { color: false })
    );
    o.loaded.unwrap()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
