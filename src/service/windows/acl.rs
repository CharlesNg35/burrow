use std::path::{Path, PathBuf};

use super::system_account;
use crate::base::failure::Failure;
use crate::base::process::run;
use crate::install::Platform;

pub(super) fn icacls(path: &Path, args: &[&str]) -> Result<(), Failure> {
    let p = path.to_string_lossy();
    let mut all = vec![p.as_ref()];
    all.extend_from_slice(args);
    run("icacls", &all).map(drop)
}

pub(super) const TRUSTED_OWNERS: [&str; 3] = [
    "S-1-5-32-544",
    "S-1-5-18",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

pub(super) fn owner_and_me(path: &Path) -> Option<(String, String)> {
    let script = format!(
        "'{{0}} {{1}}' -f (Get-Acl -LiteralPath '{}').GetOwner([System.Security.Principal.SecurityIdentifier]).Value, [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
        path.display().to_string().replace('\'', "''")
    );
    let exe = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let out = std::process::Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .env_remove("PSModulePath")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let out = String::from_utf8_lossy(&out.stdout);
    let (owner, me) = out.trim().split_once(' ')?;
    (owner.starts_with("S-1-") && me.starts_with("S-1-"))
        .then(|| (owner.to_string(), me.to_string()))
}

pub(super) fn holds_only_hand_run_folders(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|entries| {
        entries.flatten().all(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && matches!(e.file_name().to_str(), Some("instances" | "jobs"))
        })
    })
}

pub(super) fn is_link(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & REPARSE_POINT != 0)
}

pub(super) fn grant(path: &Path, account: &str, rights: &str) -> Result<(), Failure> {
    if system_account(account) || !path.exists() {
        return Ok(());
    }
    icacls(path, &["/grant", &format!("{account}:{rights}")])
}

pub(super) fn revoke(path: &Path, account: &str) -> Result<(), Failure> {
    if system_account(account) || !path.exists() || is_link(path) {
        return Ok(());
    }
    icacls(path, &["/remove:g", account])
}

pub(super) fn broad(account: &str) -> bool {
    let name = account.trim().trim_start_matches('*').to_ascii_lowercase();
    let name = name.rsplit('\\').next().unwrap_or(&name);
    [
        "everyone",
        "users",
        "authenticated users",
        "guests",
        "interactive",
        "network",
        "anonymous logon",
        "s-1-1-0",
        "s-1-5-2",
        "s-1-5-4",
        "s-1-5-7",
        "s-1-5-11",
        "s-1-5-32-545",
        "s-1-5-32-546",
    ]
    .contains(&name)
}

pub(super) fn grant_dirs(account: &str) -> Result<(), Failure> {
    let p = Platform::Windows;
    grant(&p.state_dir(), account, "(OI)(CI)RX")?;
    grant(&p.log_dir(), account, "(OI)(CI)M")?;
    grant(&crate::instance::default_dir(), account, "(OI)(CI)M")
}
