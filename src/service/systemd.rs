use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::system::{any_path_under, private_file, write_private};
use super::{Backend, Manifest, Status};
use crate::base::failure::Failure;
use crate::base::process::{run, run_ok};

fn instance_dir() -> PathBuf {
    crate::install::Platform::Linux
        .instances_dir()
        .unwrap_or_default()
}

const TMPFILES: &str = "/etc/tmpfiles.d/burrow.conf";

pub struct Systemd;

const NO_LOGIN: [&str; 3] = ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false"];
const NO_HOME: &str = "/nonexistent";
const COMMENT: &str = "burrow tunnel";

fn made_by_ensure_account(entry: &str, account: &str) -> bool {
    let fields: Vec<&str> = entry.trim_end().split(':').collect();
    matches!(
        fields.as_slice(),
        [name, _, _, _, comment, home, shell]
            if *name == account && *comment == COMMENT && *home == NO_HOME && NO_LOGIN.contains(shell)
    )
}

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
                crate::base::failure::cmd("service install --print")
            )))
        }
    }

    fn unit_path(m: &Manifest) -> PathBuf {
        PathBuf::from(format!("/etc/systemd/system/{}.service", m.name))
    }
}

fn quote_exec(p: &Path) -> String {
    quote(p).replace('$', "$$")
}

fn quote(p: &Path) -> String {
    let s = p.display().to_string().replace('%', "%%");
    if s.contains([' ', '"', '\\']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

fn env_value(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = text;
    while let Some(line) = rest.lines().next() {
        let Some((key, _)) = line.split_once("=\"") else {
            rest = rest.get(line.len() + 1..).unwrap_or("");
            continue;
        };
        let mut value = String::new();
        let mut chars = rest[key.len() + 2..].char_indices();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    end = Some(key.len() + 2 + i + 1);
                    break;
                }
                '\\' => match chars.next() {
                    Some((_, e @ ('\\' | '"' | '`' | '$'))) => value.push(e),
                    Some((_, '\n')) => {}
                    Some((_, other)) => {
                        value.push('\\');
                        value.push(other);
                    }
                    None => break,
                },
                c => value.push(c),
            }
        }
        let Some(end) = end else {
            break;
        };
        out.insert(key.to_string(), value);
        rest = &rest[end..];
        rest = rest.find('\n').map_or("", |i| &rest[i + 1..]);
    }
    out
}

const HOMES: [&str; 3] = ["/home", "/root", "/run/user"];

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
        let home_files = any_path_under(&m.files, &HOMES);
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
            "ExecStart={} {} -c {} --log-file {}{}\n",
            quote_exec(&m.binary),
            m.command(),
            quote_exec(&m.config),
            quote_exec(&m.log_file),
            m.web_state
                .as_ref()
                .map_or_else(String::new, |d| format!(" --state-dir {}", quote_exec(d)))
        ));
        s.push_str("ExecReload=/bin/kill -HUP $MAINPID\n");
        if let Some(env) = &m.env_file {
            s.push_str(&format!("EnvironmentFile={}\n", quote(env)));
        }
        s.push_str(&format!(
            "Restart=on-failure\nRestartSec=5\nRestartPreventExitStatus={} {}\nRestartForceExitStatus=SIGHUP\nLimitNOFILE=1048576\nUMask=0077\n",
            crate::base::failure::EXIT_USAGE,
            crate::base::failure::EXIT_CONFIG
        ));
        s.push_str(&format!("ReadWritePaths={}\n", instance_dir().display()));
        if m.admin_writable {
            s.push_str(&format!("ReadWritePaths={}\n", quote(m.folder())));
        }
        s.push_str(&format!(
            "LogsDirectory=burrow/{}\nLogsDirectoryMode=0750\n",
            m.short()
        ));
        if m.web_state.is_some() {
            s.push_str(&format!(
                "StateDirectory=burrow/{}\nStateDirectoryMode=0700\n",
                m.short()
            ));
        }
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
        let shell = NO_LOGIN
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
                NO_HOME,
                "--shell",
                shell,
                "--comment",
                COMMENT,
                account,
            ],
        )
        .map_err(|f| {
            f.hint("burrow creates the account with useradd; create it yourself or pass --run-as")
        })?;
        Ok(true)
    }

    fn made_for_burrow(&self, account: &str) -> bool {
        run("getent", &["passwd", account]).is_ok_and(|e| made_by_ensure_account(&e, account))
    }

    fn remove_account(&self, account: &str) -> Result<(), Failure> {
        run("userdel", &[account])?;
        if run_ok("getent", &["group", account]) {
            let _ = run("groupdel", &[account]);
        }
        Ok(())
    }

    fn low_ports(&self, m: &Manifest) -> bool {
        m.low_ports || m.account == "root"
    }

    fn state_folder(&self, _m: &Manifest) -> Result<Vec<PathBuf>, Failure> {
        Ok(Vec::new())
    }

    fn hidden(&self, file: &Path) -> Option<String> {
        ["/tmp", "/var/tmp"]
            .iter()
            .find(|d| file.starts_with(d))
            .map(|d| format!("{d} is private to the service, so it cannot see this file"))
    }

    fn cannot_use(&self, m: &Manifest, file: &Path) -> Option<String> {
        if file.is_relative() {
            Some("a service needs an absolute path".into())
        } else if let Some(why) = self.hidden(file) {
            Some(why)
        } else if HOMES.iter().any(|h| file.starts_with(h)) && !any_path_under(&m.files, &HOMES) {
            Some("it is in a home folder, which the service was installed without access to".into())
        } else if !self.can_read(&m.account, file) {
            Some(format!("its account {} cannot read it", m.account))
        } else {
            None
        }
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
        for dir in ["/etc/burrow", "/var/lib/burrow", "/var/lib/burrow/services"] {
            super::system::make_dir(Path::new(dir), 0o755, None)?;
        }
        super::system::write_file(
            Path::new(TMPFILES),
            format!("d {} 1777 root root -\n", instance_dir().display()).as_bytes(),
        )?;
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
        super::system::write_file(&path, self.render(m).as_bytes())?;
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
        if super::all_manifests().iter().all(|o| o.name == m.name)
            && super::stray::strays().is_empty()
        {
            let _ = std::fs::remove_file(TMPFILES);
        }
        Ok(())
    }

    fn start(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["start", &m.name])
            .map(drop)
            .map_err(|f| f.hint(super::logs_hint(&m.name)))
    }

    fn stop(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["stop", &m.name]).map(drop)
    }

    fn reload(&self, m: &Manifest) -> Result<(), Failure> {
        run("systemctl", &["reload", &m.name]).map(drop)
    }

    fn units(&self) -> Vec<super::stray::Unit> {
        super::stray::read_units(
            Path::new("/etc/systemd/system"),
            "",
            ".service",
            super::stray::systemd_unit,
        )
    }

    fn manual_stop(&self, name: &str) -> String {
        format!("sudo systemctl stop {name}")
    }

    fn journal_hint(&self, m: &Manifest) -> Option<String> {
        Some(format!("sudo journalctl -u {} -n 50", m.name))
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
            code if super::is_config_exit(code) => super::config_exit_detail(&m.name),
            "203" => format!(
                "{} {}: systemd could not run {} as {} (203/EXEC); the account may not reach the binary",
                get("ActiveState"),
                get("SubState"),
                m.binary.display(),
                m.account
            ),
            code if !code.is_empty() && code != "0" => format!(
                "{} {} (last exit code {code}, result {})",
                get("ActiveState"),
                get("SubState"),
                get("Result")
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
    fn only_an_account_shaped_like_the_one_burrow_creates_counts_as_burrows() {
        let ours = "burrow:x:998:998:burrow tunnel:/nonexistent:/usr/sbin/nologin\n";
        assert!(made_by_ensure_account(ours, "burrow"));
        assert!(made_by_ensure_account(
            "burrow:x:998:998:burrow tunnel:/nonexistent:/bin/false",
            "burrow"
        ));
        assert!(!made_by_ensure_account(ours, "other"));
        for theirs in [
            "burrow:x:1001:1001:Burrow User:/home/burrow:/bin/bash",
            "burrow:x:998:998:burrow tunnel:/home/burrow:/usr/sbin/nologin",
            "burrow:x:998:998:burrow tunnel:/nonexistent:/bin/sh",
            "burrow:x:998:998::/nonexistent:/usr/sbin/nologin",
            "",
        ] {
            assert!(!made_by_ensure_account(theirs, "burrow"), "{theirs}");
        }
    }

    #[test]
    fn the_environment_file_reads_back_what_was_written() {
        let env = BTreeMap::from([
            ("HOME_SECRET".to_string(), "a\"b\\c\nd=e".to_string()),
            (
                "PEM".to_string(),
                "-----BEGIN-----\nAAA\n-----END-----\n".to_string(),
            ),
            ("SHELLY".to_string(), "$HOME `x` \\n".to_string()),
            ("PLAIN".to_string(), "x".to_string()),
        ]);
        let text: String = env
            .iter()
            .map(|(k, v)| format!("{k}={}\n", env_value(v)))
            .collect();
        assert!(text.contains("PEM=\"-----BEGIN-----\nAAA\n"), "{text}");
        assert_eq!(parse_env(&text), env);
        assert_eq!(parse_env(&format!("# note\n{text}")), env);
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
                role: crate::install::Role::parse(role).unwrap(),
                config: "/etc/burrow/home/server.yaml".into(),
                source_config: "/home/me/home.yaml".into(),
                env_file: None,
                log_file: "/var/log/burrow/home/home.log".into(),
                binary: "/usr/local/bin/burrow".into(),
                copied_binary: true,
                account: "burrow-home".into(),
                created_account: true,
                low_ports: false,
                files: vec![],
                version: "0.1.0".into(),
                admin_writable: false,
                web_state: None,
            };
            let unit = Systemd.render(&m);
            assert!(
                unit.contains(&format!(
                    "ExecStart=/usr/local/bin/burrow {command} -c /etc/burrow/home/server.yaml --log-file"
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

    fn server_manifest() -> Manifest {
        Manifest {
            name: "burrow-home".into(),
            role: crate::install::Role::Server,
            config: "/etc/burrow/home/server.yaml".into(),
            source_config: "/home/me/home.yaml".into(),
            env_file: Some("/etc/burrow/home.env".into()),
            log_file: "/var/log/burrow/home/home.log".into(),
            binary: "/usr/local/bin/burrow".into(),
            copied_binary: true,
            account: "burrow-home".into(),
            created_account: true,
            low_ports: false,
            files: vec![],
            version: "0.1.0".into(),
            admin_writable: false,
            web_state: None,
        }
    }

    #[test]
    fn the_server_unit_is_unchanged_byte_for_byte() {
        let unit = Systemd.render(&server_manifest());
        assert_eq!(unit, GOLDEN_UNIT);
        assert!(unit.contains(&format!(
            "RestartPreventExitStatus={} {}\n",
            crate::base::failure::EXIT_USAGE,
            crate::base::failure::EXIT_CONFIG
        )));
    }

    #[test]
    fn a_writable_config_folder_is_the_only_extra_writable_path() {
        let m = Manifest {
            admin_writable: true,
            ..server_manifest()
        };
        let unit = Systemd.render(&m);
        let expected = GOLDEN_UNIT.replace(
            "ReadWritePaths=/run/burrow-instances\n",
            "ReadWritePaths=/run/burrow-instances\nReadWritePaths=/etc/burrow/home\n",
        );
        assert_eq!(unit, expected);
    }

    #[test]
    fn percent_and_dollar_in_paths_reach_the_program_unchanged() {
        let m = Manifest {
            config: "/etc/burrow/a%b$c d.yaml".into(),
            env_file: Some("/etc/burrow/100%.env".into()),
            admin_writable: true,
            ..server_manifest()
        };
        let unit = Systemd.render(&m);
        assert!(
            unit.contains("server -c \"/etc/burrow/a%%b$$c d.yaml\" --log-file"),
            "{unit}"
        );
        assert!(
            unit.contains("EnvironmentFile=/etc/burrow/100%%.env\n"),
            "{unit}"
        );
        let m = Manifest {
            config: "/srv/$x/server.yaml".into(),
            admin_writable: true,
            ..server_manifest()
        };
        let unit = Systemd.render(&m);
        assert!(unit.contains("-c /srv/$$x/server.yaml "), "{unit}");
        assert!(unit.contains("ReadWritePaths=/srv/$x\n"), "{unit}");
    }

    const GOLDEN_UNIT: &str = "[Unit]\nDescription=burrow server \"home\"\nDocumentation=https://github.com/charlesng35/burrow\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nUser=burrow-home\nExecStart=/usr/local/bin/burrow server -c /etc/burrow/home/server.yaml --log-file /var/log/burrow/home/home.log\nExecReload=/bin/kill -HUP $MAINPID\nEnvironmentFile=/etc/burrow/home.env\nRestart=on-failure\nRestartSec=5\nRestartPreventExitStatus=2 3\nRestartForceExitStatus=SIGHUP\nLimitNOFILE=1048576\nUMask=0077\nReadWritePaths=/run/burrow-instances\nLogsDirectory=burrow/home\nLogsDirectoryMode=0750\nNoNewPrivileges=yes\nProtectSystem=strict\nProtectHome=yes\nPrivateTmp=yes\nPrivateDevices=yes\nDevicePolicy=closed\nPrivateUsers=yes\nProtectClock=yes\nProtectHostname=yes\nProtectKernelTunables=yes\nProtectKernelModules=yes\nProtectKernelLogs=yes\nProtectControlGroups=yes\nProtectProc=invisible\nProcSubset=pid\nRestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK\nRestrictNamespaces=yes\nRestrictRealtime=yes\nRestrictSUIDSGID=yes\nLockPersonality=yes\nMemoryDenyWriteExecute=yes\nRemoveIPC=yes\nSystemCallArchitectures=native\nSystemCallFilter=@system-service\nSystemCallFilter=~@privileged @resources\nSystemCallErrorNumber=EPERM\nCapabilityBoundingSet=\nAmbientCapabilities=\n\n[Install]\nWantedBy=multi-user.target\n";
}
