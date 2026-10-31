use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::system::{make_dir, private_file, write_private};
use super::{Backend, Manifest, Platform, Status};
use crate::base::failure::Failure;
use crate::base::process::{run, run_ok};

pub struct Launchd;

const NO_LOGIN: &str = "/usr/bin/false";
const NO_HOME: &str = "/var/empty";
const COMMENT: &str = "burrow tunnel";

fn made_by_ensure_account(dscl: &str) -> bool {
    let mut values = BTreeMap::new();
    let mut last = None;
    for line in dscl.lines() {
        if let Some(more) = line.strip_prefix(' ') {
            if let Some(k) = last {
                values.insert(k, more.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            values.insert(k.trim(), v.trim());
            last = Some(k.trim());
        }
    }
    values.get("UserShell") == Some(&NO_LOGIN)
        && values.get("NFSHomeDirectory") == Some(&NO_HOME)
        && values.get("RealName") == Some(&COMMENT)
}

fn label(m: &Manifest) -> String {
    format!("com.github.charlesng35.{}", m.name)
}

fn target(m: &Manifest) -> String {
    format!("system/{}", label(m))
}

fn loaded(m: &Manifest) -> bool {
    run_ok("launchctl", &["print", &target(m)])
}

fn plist_path(m: &Manifest) -> PathBuf {
    PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", label(m)))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn plist(m: &Manifest) -> String {
    let mut args = vec![
        m.binary.display().to_string(),
        m.command().to_string(),
        "-c".into(),
        m.config.display().to_string(),
        "--log-file".into(),
        m.log_file.display().to_string(),
    ];
    if let Some(env) = &m.env_file {
        args.push("--env-file".into());
        args.push(env.display().to_string());
    }
    if let Some(dir) = &m.web_state {
        args.push("--state-dir".into());
        args.push(dir.display().to_string());
    }
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s.push_str(&format!(
        "  <key>Label</key>\n  <string>{}</string>\n",
        xml(&label(m))
    ));
    s.push_str("  <key>ProgramArguments</key>\n  <array>\n");
    for a in &args {
        s.push_str(&format!("    <string>{}</string>\n", xml(a)));
    }
    s.push_str("  </array>\n");
    if m.account != "root" {
        s.push_str(&format!(
            "  <key>UserName</key>\n  <string>{0}</string>\n",
            xml(&m.account)
        ));
    }
    s.push_str("  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n");
    s.push_str("  <key>ThrottleInterval</key>\n  <integer>10</integer>\n  <key>Umask</key>\n  <integer>63</integer>\n");
    s.push_str("  <key>SoftResourceLimits</key>\n  <dict>\n    <key>NumberOfFiles</key>\n    <integer>10240</integer>\n  </dict>\n");
    s.push_str("</dict>\n</plist>\n");
    s
}

fn free_id() -> Result<u32, Failure> {
    let taken = |path: &str, key: &str| -> Vec<u32> {
        run("dscl", &[".", "-list", path, key])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_whitespace().last()?.parse().ok())
            .collect()
    };
    let mut used = taken("/Users", "UniqueID");
    used.extend(taken("/Groups", "PrimaryGroupID"));
    (300..500)
        .rev()
        .find(|id| !used.contains(id))
        .ok_or_else(|| {
            Failure::new("no free system id between 300 and 499 for the service account")
                .hint("pass --run-as with an existing account")
        })
}

impl Backend for Launchd {
    fn kind(&self) -> &'static str {
        "launchd"
    }

    fn low_ports(&self, m: &Manifest) -> bool {
        m.account == "root"
    }

    fn wildcard_low_ports(&self) -> bool {
        true
    }

    fn render(&self, m: &Manifest) -> String {
        plist(m)
    }

    fn ensure_account(&self, account: &str) -> Result<bool, Failure> {
        if account == "root" || run_ok("id", &["-u", account]) {
            return Ok(false);
        }
        let id = free_id()?.to_string();
        let group = format!("/Groups/{account}");
        let user = format!("/Users/{account}");
        let steps: [&[&str]; 10] = [
            &[".", "-create", &group],
            &[".", "-create", &group, "PrimaryGroupID", &id],
            &[".", "-create", &group, "Password", "*"],
            &[".", "-create", &user],
            &[".", "-create", &user, "UniqueID", &id],
            &[".", "-create", &user, "PrimaryGroupID", &id],
            &[".", "-create", &user, "UserShell", NO_LOGIN],
            &[".", "-create", &user, "NFSHomeDirectory", NO_HOME],
            &[".", "-create", &user, "RealName", COMMENT],
            &[".", "-create", &user, "IsHidden", "1"],
        ];
        for args in steps {
            if let Err(f) = run("dscl", args) {
                let _ = self.remove_account(account);
                return Err(f.hint(
                    "burrow creates the account with dscl; create it yourself or pass --run-as",
                ));
            }
        }
        Ok(true)
    }

    fn made_for_burrow(&self, account: &str) -> bool {
        run(
            "dscl",
            &[
                ".",
                "-read",
                &format!("/Users/{account}"),
                "UserShell",
                "NFSHomeDirectory",
                "RealName",
            ],
        )
        .is_ok_and(|t| made_by_ensure_account(&t))
    }

    fn remove_account(&self, account: &str) -> Result<(), Failure> {
        let user = run("dscl", &[".", "-delete", &format!("/Users/{account}")]);
        let _ = run("dscl", &[".", "-delete", &format!("/Groups/{account}")]);
        let _ = run("dscacheutil", &["-flushcache"]);
        user.map(drop)
    }

    fn can_read(&self, account: &str, path: &Path) -> bool {
        if account == "root" {
            return path.exists();
        }
        let p = path.to_string_lossy();
        run_ok(
            "/usr/bin/sudo",
            &["-n", "-u", account, "/bin/test", "-r", &p],
        )
    }

    fn secure_file(&self, path: &Path, account: &str) -> Result<(), Failure> {
        private_file(path, Some(account))
    }

    fn prepare_dirs(&self, _account: &str) -> Result<(), Failure> {
        for (dir, mode) in Platform::MacOs.launchd_folders() {
            make_dir(&dir, mode, None)?;
        }
        Ok(())
    }

    fn read_env(&self, m: &Manifest) -> BTreeMap<String, String> {
        super::read_json_env(m)
    }

    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
        let Some(path) = &m.env_file else {
            return Ok(());
        };
        write_private(
            path,
            &serde_json::to_string_pretty(env).expect("env serializes"),
        )?;
        self.secure_file(path, &m.account)
    }

    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        if let Some(dir) = m.log_file.parent() {
            let owner = (m.account != "root").then_some(m.account.as_str());
            make_dir(dir, 0o750, owner)?;
            let (uid, gid) = match owner {
                Some(account) => crate::base::process::account_ids(account)?,
                None => (0, 0),
            };
            if owner.is_none() {
                std::os::unix::fs::lchown(dir, Some(0), Some(0)).map_err(|e| {
                    Failure::new(format!("cannot give {} to root: {e}", dir.display()))
                })?;
            }
            let kept = crate::base::logfile::all_files(&m.log_file);
            for f in kept.iter().filter(|f| f.symlink_metadata().is_ok()) {
                if f.symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    let _ = std::fs::remove_file(f);
                    continue;
                }
                match super::system::give_plain_file(f, uid, gid) {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = std::fs::remove_file(f);
                    }
                    Err(e) => {
                        return Err(Failure::new(format!(
                            "cannot give {} to {}: {e}",
                            f.display(),
                            m.account
                        )));
                    }
                }
            }
        }
        let _ = run("launchctl", &["bootout", &target(m)]);
        let path = plist_path(m);
        write_private(&path, &self.render(m))?;
        private_file(&path, None)?;
        run("launchctl", &["enable", &target(m)])?;
        Ok(())
    }

    fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
        let _ = run("launchctl", &["bootout", &target(m)]);
        let path = plist_path(m);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| Failure::new(format!("cannot remove {}: {e}", path.display())))?;
        }
        Ok(())
    }

    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        let hint = |f: Failure| f.hint(super::logs_hint(&m.name));
        if loaded(m) {
            return run("launchctl", &["kickstart", &target(m)])
                .map(drop)
                .map_err(hint);
        }
        let path = plist_path(m).to_string_lossy().into_owned();
        let mut last = None;
        for _ in 0..20 {
            match run("launchctl", &["bootstrap", "system", &path]) {
                Ok(_) => return Ok(()),
                Err(_) if loaded(m) => return Ok(()),
                Err(f) => last = Some(f),
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(hint(last.expect("at least one attempt")))
    }

    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        if !loaded(m) {
            return Ok(());
        }
        run("launchctl", &["bootout", &target(m)])?;
        for _ in 0..40 {
            if !loaded(m) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(Failure::new(format!(
            "{} did not stop within 10 seconds",
            m.name
        )))
    }

    fn reload(&self, m: &Manifest) -> Result<(), Failure> {
        run("launchctl", &["kill", "SIGHUP", &target(m)]).map(drop)
    }

    fn units(&self) -> Vec<super::stray::Unit> {
        super::stray::read_units(
            Path::new("/Library/LaunchDaemons"),
            "com.github.charlesng35.",
            ".plist",
            super::stray::launchd_job,
        )
    }

    fn manual_stop(&self, name: &str) -> String {
        format!("sudo launchctl bootout system/com.github.charlesng35.{name}")
    }

    fn status(&self, m: &Manifest) -> Status {
        let installed = plist_path(m).exists();
        let Ok(text) = run("launchctl", &["print", &target(m)]) else {
            return Status {
                installed,
                running: false,
                pid: None,
                detail: "not loaded".into(),
            };
        };
        let field = |k: &str| {
            text.lines()
                .find_map(|l| {
                    l.trim()
                        .strip_prefix(k)?
                        .trim_start()
                        .strip_prefix('=')
                        .map(|v| v.trim().to_string())
                })
                .unwrap_or_default()
        };
        let state = field("state");
        let code = field("last exit code");
        let detail = match code.as_str() {
            c if super::is_config_exit(c) => super::config_exit_detail(&m.name),
            c if !c.is_empty() && c != "0" && !c.starts_with('(') => {
                format!("{state} (last exit code {c})")
            }
            _ => state.clone(),
        };
        Status {
            installed,
            running: state == "running",
            pid: field("pid").parse().ok(),
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_account_shaped_like_the_one_burrow_creates_counts_as_burrows() {
        assert!(made_by_ensure_account(
            "NFSHomeDirectory: /var/empty\nRealName:\n burrow tunnel\nUserShell: /usr/bin/false\n"
        ));
        assert!(made_by_ensure_account(
            "NFSHomeDirectory: /var/empty\nRealName: burrow tunnel\nUserShell: /usr/bin/false\n"
        ));
        for theirs in [
            "NFSHomeDirectory: /Users/burrow\nRealName:\n burrow tunnel\nUserShell: /usr/bin/false\n",
            "NFSHomeDirectory: /var/empty\nRealName:\n Burrow\nUserShell: /usr/bin/false\n",
            "NFSHomeDirectory: /var/empty\nRealName:\n burrow tunnel\nUserShell: /bin/zsh\n",
            "NFSHomeDirectory: /var/empty\nUserShell: /usr/bin/false\n",
            "",
        ] {
            assert!(!made_by_ensure_account(theirs), "{theirs}");
        }
    }

    #[test]
    fn plist_runs_the_service_account_and_escapes_values() {
        let m = Manifest {
            name: "burrow-home".into(),
            role: crate::install::Role::Client,
            config: "/etc/burrow/home/server.yaml".into(),
            source_config: "/Users/me/home.yaml".into(),
            env_file: None,
            log_file: "/var/log/burrow/home/home.log".into(),
            binary: "/usr/local/bin/burrow".into(),
            copied_binary: true,
            account: "_burrow".into(),
            created_account: true,
            low_ports: false,
            files: vec![],
            version: "0.1.0".into(),
            admin_writable: false,
            web_state: None,
        };
        let p = plist(&Manifest {
            env_file: Some("/etc/burrow/home.env".into()),
            ..m.clone()
        });
        assert!(p.contains("<string>com.github.charlesng35.burrow-home</string>"));
        assert!(p.contains("<key>UserName</key>\n  <string>_burrow</string>"));
        assert!(p.contains(
            "<string>--log-file</string>\n    <string>/var/log/burrow/home/home.log</string>"
        ));
        assert!(!p.contains("EnvironmentVariables"));
        assert!(
            p.contains("<string>--env-file</string>\n    <string>/etc/burrow/home.env</string>")
        );
        assert!(p.contains("<string>/usr/local/bin/burrow</string>\n    <string>client</string>"));
        let visitor = Manifest {
            role: crate::install::Role::Visitor,
            ..m
        };
        assert!(
            plist(&visitor)
                .contains("<string>/usr/local/bin/burrow</string>\n    <string>visit</string>")
        );
        assert!(!p.contains("--state-dir"));
        let web = plist(&Manifest {
            web_state: Some("/Library/Application Support/burrow/home".into()),
            ..visitor
        });
        assert!(web.contains(
            "<string>--state-dir</string>\n    <string>/Library/Application Support/burrow/home</string>"
        ));
    }
}
