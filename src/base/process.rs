use std::process::Command;

use crate::base::failure::Failure;

pub fn run(program: &str, args: &[&str]) -> Result<String, Failure> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Failure::new(format!("cannot run {program}: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.trim();
        Err(Failure::new(format!(
            "{program} {} failed: {}",
            args.join(" "),
            if msg.is_empty() { "no output" } else { msg }
        )))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub fn run_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(unix)]
pub fn account_ids(account: &str) -> Result<(u32, u32), Failure> {
    let read = |flag: &str| -> Result<u32, Failure> {
        run("id", &[flag, account])?
            .trim()
            .parse()
            .map_err(|_| Failure::new(format!("cannot read the ids of {account}")))
    };
    Ok((read("-u")?, read("-g")?))
}
