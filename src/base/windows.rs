use std::path::Path;
use std::process::Command;

use super::failure::Failure;

pub const SYSTEM: &str = "*S-1-5-18";
pub const ADMINS: &str = "*S-1-5-32-544";
pub const USERS: &str = "*S-1-5-32-545";

pub fn bare(sid: &str) -> &str {
    sid.trim_start_matches('*')
}

pub fn icacls(path: &Path, args: &[&str]) -> Result<(), Failure> {
    super::process::output_of(Command::new("icacls").arg(path).args(args)).map(drop)
}
