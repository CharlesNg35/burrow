use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{tools, ui};

pub fn shots_dir(root: &Path) -> PathBuf {
    std::env::var_os("BURROW_E2E_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("tmp/admin-ui-shots"))
}

pub fn test_args(filter: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "test",
        "--locked",
        "--features",
        "admin-test-hooks",
        "--test",
        "admin_e2e",
        "--",
        "--test-threads=1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(f) = filter {
        args.push(f.to_string());
    }
    args
}

pub fn main(root: &Path, args: &[String]) -> Result<(), String> {
    let update = args.iter().any(|a| a == "--update-baselines");
    let filter = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);
    if let Some(unknown) = args
        .iter()
        .find(|a| a.starts_with("--") && *a != "--update-baselines")
    {
        return Err(format!(
            "unknown option {unknown}; usage: cargo xtask e2e [flow-filter] [--update-baselines]"
        ));
    }
    if std::env::var_os("BURROW_E2E_CHROME").is_none()
        && std::env::var("BURROW_E2E_SYSTEM_CHROME").as_deref() != Ok("1")
    {
        let chrome = tools::chrome(root)?;
        println!("Chrome for Testing {}: {}", tools::CHROME, chrome.display());
    }
    ui::main(root, &["--release".to_string()])?;
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root).args(test_args(filter));
    if update {
        cmd.env("BURROW_E2E_UPDATE_BASELINES", "1");
    }
    if filter.is_none() {
        cmd.env("BURROW_E2E_FRESH", "1");
    }
    let status = cmd.status().map_err(|e| format!("cargo test: {e}"))?;
    let sheet = shots_dir(root).join("index.html");
    println!("contact sheet: {}", sheet.display());
    if !status.success() {
        return Err("the admin e2e flows failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_chrome_at_a_time_and_the_filter_goes_last() {
        let a = test_args(Some("e3"));
        assert!(a.contains(&"--test-threads=1".to_string()));
        assert!(a.contains(&"admin-test-hooks".to_string()));
        assert_eq!(a.last().map(String::as_str), Some("e3"));
        assert_eq!(
            test_args(None).last().map(String::as_str),
            Some("--test-threads=1")
        );
    }
}
