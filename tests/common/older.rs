use std::path::PathBuf;

pub const COMPAT_BIN: &str = "BURROW_COMPAT_BIN";

pub fn older() -> Option<PathBuf> {
    let bin = std::env::var_os(COMPAT_BIN).map(PathBuf::from);
    if bin.is_none() {
        println!(
            "skipping: {COMPAT_BIN} is not set; run cargo xtask compat to test against the last release"
        );
    }
    bin
}
