use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::system::{all_paths_under, private_file, run, run_ok, write_private};
use super::{Backend, Manifest, Status};
use crate::failure::Failure;

pub const INSTANCE_DIR: &str = "/run/burrow-instances";
const TMPFILES: &str = "/etc/tmpfiles.d/burrow.conf";

pub struct Systemd;

impl Systemd {
    pub fn detect() -> Result<Systemd, Failure> {
        if Path::new("/run/systemd/system").is_dir() {
            Ok(Systemd)
        } else {
            Err(Failure::new(
                "this machine does not run systemd, which burrow service needs on Linux",
            )
            .hint(format!(
                "print a unit for your own setup with: {}, or run burrow in Docker",
                crate::failure::cmd("service install --print")
            )))
        }
    }

    fn unit_path(m: &Manifest) -> PathBuf {
        PathBuf::from(format!("/etc/systemd/system/{}.service", m.name))
    }
}

fn quote(p: &Path) -> String {
    let s = p.display().to_string();
    if s.contains([' ', '"', '\\']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

fn env_value(v: &str) -> String {
    format!(
        "\"{}\"",
        v.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, quoted) = line.split_once('=')?;
            let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
            let mut value = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                value.push(match c {
                    '\\' => match chars.next()? {
                        'n' => '\n',
                        other => other,
                    },
                    c => c,
                });
            }
            Some((key.to_string(), value))
        })
        .collect()
}

impl Backend for Systemd {
    fn kind(&self) -> &'static str {
        "systemd"
    }

    fn render(&self, m: &Manifest) -> String {
        let root = m.account == "root";
        let caps = if m.low_ports {
            "CAP_NET_BIND_SERVICE"
        } else {
            ""
        };
        let home_files = all_paths_under(&m.files, &["/home", "/root", "/run/user"]);
        let mut s = String::new();
        s.push_str(&format!(
            "[Unit]\nDescription=burrow {} \"{}\"\nDocumentation=https://github.com/charlesng35/burrow\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n",
            m.role,
            m.short()
        ));
        if !root {
            s.push_str(&format!("User={}\n", m.account));
        }
        s.push_str(&format!(
            "ExecStart={} {} -c {} --log-file {}\n",
            quote(&m.binary),
            m.command(),
            quote(&m.config),
            quote(&m.log_file)
        ));
        s.push_str("ExecReload=/bin/kill -HUP $MAINPID\n");
        if let Some(env) = &m.env_file {
            s.push_str(&format!("EnvironmentFile={}\n", quote(env)));
        }
        s.push_str("Restart=on-failure\nRestartSec=5\nRestartPreventExitStatus=2 3\nLimitNOFILE=1048576\nUMask=0077\n");
        s.push_str(&format!(
            "ReadWritePaths={INSTANCE_DIR}\nLogsDirectory=burrow/{}\nLogsDirectoryMode=0750\n",
            m.short()
        ));
        s.push_str("NoNewPrivileges=yes\nProtectSystem=strict\n");
        s.push_str(if home_files {
            "ProtectHome=read-only\n"
        } else {
            "ProtectHome=yes\n"
        });
        s.push_str("PrivateTmp=yes\nPrivateDevices=yes\nDevicePolicy=closed\n");
        if !m.low_ports && !root {
            s.push_str("PrivateUsers=yes\n");
        }
        s.push_str("ProtectClock=yes\nProtectHostname=yes\nProtectKernelTunables=yes\nProtectKernelModules=yes\nProtectKernelLogs=yes\nProtectControlGroups=yes\nProtectProc=invisible\nProcSubset=pid\n");
        s.push_str("RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK\nRestrictNamespaces=yes\nRestrictRealtime=yes\nRestrictSUIDSGID=yes\nLockPersonality=yes\nMemoryDenyWriteExecute=yes\nRemoveIPC=yes\n");
        s.push_str("SystemCallArchitectures=native\nSystemCallFilter=@system-service\nSystemCallFilter=~@privileged @resources\nSystemCallErrorNumber=EPERM\n");
        s.push_str(&format!(
            "CapabilityBoundingSet={caps}\nAmbientCapabilities={caps}\n"
        ));
        s.push_str("\n[Install]\nWantedBy=multi-user.target\n");
        s
    }

    fn ensure_account(&self, account: &str) -> Result<bool, Failure> {
        if account == "root" || run_ok("id", &["-u", account]) {
            return Ok(false);
        }
        let shell = ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .unwrap_or("/bin/false");
        run(
            "useradd",
            &[
                "--system",
                "--user-group",
                "--no-create-home",
                "--home-dir",
                "/nonexistent",
                "--shell",
                shell,
                "--comment",
                "burrow tunnel",
                account,
            ],
        )
        .map_err(|f| {
            f.hint("burrow creates the account with useradd; create it yourself or pass --run-as")
        })?;
        Ok(true)
    }

    fn remove_account(&self, account: &str) -> Result<(), Failure> {
        run("userdel", &[account])?;
        if run_ok("getent", &["group", account]) {
            let _ = run("groupdel", &[account]);
        }
        Ok(())
    }

    fn can_read(&self, account: &str, path: &Path) -> bool {
        if account == "root" {
            return path.exists();
        }
        let p = path.to_string_lossy();
        run_ok("runuser", &["-u", account, "--", "test", "-r", &p])
    }

    fn secure_file(&self, path: &Path, account: &str) -> Result<(), Failure> {
        private_file(path, Some(account))
    }

    fn prepare_dirs(&self, _account: &str) -> Result<(), Failure> {
        for dir in ["/etc/burrow", "/var/lib/burrow/services"] {
            std::fs::create_dir_all(dir)
                .map_err(|e| Failure::new(format!("cannot create {dir}: {e}")))?;
        }
        std::fs::write(TMPFILES, format!("d {INSTANCE_DIR} 1777 root root -\n"))
            .map_err(|e| Failure::new(format!("cannot write {TMPFILES}: {e}")))?;
        run("systemd-tmpfiles", &["--create", TMPFILES])?;
        Ok(())
    }

    fn read_env(&self, m: &Manifest) -> BTreeMap<String, String> {
        m.env_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| parse_env(&text))
            .unwrap_or_default()
    }

    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
        let Some(path) = &m.env_file else {
            return Ok(());
        };
        let text: String = env
            .iter()
            .map(|(k, v)| format!("{k}={}\n", env_value(v)))
            .collect();
        write_private(path, &text)
    }

    fn install(&self, m: &Manifest) -> Result<(), Failure> {
        let path = Self::unit_path(m);
        std::fs::write(&path, self.render(m))
            .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
        run("systemctl", &["daemon-reload"])?;
        run("systemctl", &["enable", "--quiet", &m.name])?;
        Ok(())
    }

    fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
        let _ = run("systemctl", &["disable", "--quiet", &m.name]);
        let path = Self::unit_path(m);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| Failure::new(format!("cannot remove {}: {e}", path.display())))?;
        }
        run("systemctl", &["daemon-reload"])?;
        let _ = run("systemctl", &["reset-failed", &m.name]);
        if super::all_manifests().iter().all(|o| o.name == m.name) {
            let _ = std::fs::remove_file(TMPFILES);
        }
        Ok(())
    }

    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["start", &m.name])
            .map(drop)
            .map_err(|f| {
                f.hint(format!(
                    "see why with: {}",
                    crate::failure::admin(&format!("service logs {}", m.name))
                ))
            })
    }

    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["stop", &m.name]).map(drop)
    }

    fn reload(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["reload", &m.name]).map(drop)
    }

    fn status(&self, m: &Manifest) -> Status {
        let text = run(
            "systemctl",
            &[
                "show",
                &m.name,
                "-p",
                "LoadState",
                "-p",
                "ActiveState",
                "-p",
                "SubState",
                "-p",
                "MainPID",
                "-p",
                "Result",
                "-p",
                "ExecMainStatus",
            ],
        )
        .unwrap_or_default();
        let get = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .unwrap_or("")
                .to_string()
        };
        let pid = get("MainPID").parse::<u32>().ok().filter(|p| *p > 0);
        let detail = match get("ExecMainStatus").as_str() {
            "3" => format!(
                "the config has errors (exit 3); fix it, then: {}",
                crate::failure::admin(&format!("service start {}", m.name))
            ),
            code if !code.is_empty() && code != "0" => format!(
                "{} {} (last exit code {code})",
                get("ActiveState"),
                get("SubState")
            ),
            _ => format!("{} {}", get("ActiveState"), get("SubState")),
        };
        Status {
            installed: get("LoadState") == "loaded",
            running: get("ActiveState") == "active",
            pid,
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_file_reads_back_what_was_written() {
        let env = BTreeMap::from([
            ("HOME_SECRET".to_string(), "a\"b\\c\nd=e".to_string()),
            ("PLAIN".to_string(), "x".to_string()),
        ]);
        let text: String = env
            .iter()
            .map(|(k, v)| format!("{k}={}\n", env_value(v)))
            .collect();
        assert_eq!(parse_env(&text), env);
    }

    #[test]
    fn units_run_the_command_of_each_role() {
        for (role, command) in [
            ("server", "server"),
            ("client", "client"),
            ("visitor", "visit"),
        ] {
            let m = Manifest {
                name: "burrow-home".into(),
                role: role.into(),
                config: "/etc/burrow/home.yaml".into(),
                source_config: "/home/me/home.yaml".into(),
                env_file: None,
                log_file: "/var/log/burrow/home/home.log".into(),
                binary: "/usr/local/bin/burrow".into(),
                copied_binary: true,
                account: "burrow-home".into(),
                created_account: true,
                low_ports: false,
                files: vec![],
                version: "0.2.0".into(),
            };
            let unit = Systemd.render(&m);
            assert!(
                unit.contains(&format!(
                    "ExecStart=/usr/local/bin/burrow {command} -c /etc/burrow/home.yaml --log-file"
                )),
                "{unit}"
            );
            assert!(unit.contains(&format!("Description=burrow {role} \"home\"")));
            assert!(
                unit.contains("ExecReload=/bin/kill -HUP $MAINPID"),
                "{role}"
            );
        }
    }
}
