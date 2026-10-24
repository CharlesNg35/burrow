use std::io::Write;
use std::path::{Path, PathBuf};

use super::{say, service_name, system};
use crate::base::failure::Failure;
use crate::install::{Manifest, all_manifests, read_manifest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub label: String,
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub name: String,
    pub label: String,
    pub config: Option<PathBuf>,
    pub running: bool,
    pub pid: Option<u32>,
    pub stop: String,
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub fn burrow_command(args: &[String]) -> Option<Option<PathBuf>> {
    let program = args.first()?;
    let file = program.rsplit(['/', '\\']).next().unwrap_or(program);
    if !(file == "burrow" || file.eq_ignore_ascii_case("burrow.exe")) {
        return None;
    }
    if !matches!(args.get(1)?.as_str(), "server" | "client" | "visit") {
        return None;
    }
    let mut rest = args[2..].iter();
    while let Some(arg) = rest.next() {
        if arg == "-c" || arg == "--config" {
            return Some(rest.next().map(PathBuf::from));
        }
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(Some(PathBuf::from(path)));
        }
    }
    Some(None)
}

#[cfg(any(target_os = "linux", test))]
pub fn systemd_unit(text: &str) -> Option<Option<PathBuf>> {
    let line = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ExecStart="))
        .find(|v| !v.trim().is_empty())?;
    let line = line.trim_start_matches(['-', '@', '+', '!', ':']);
    burrow_command(&shlex::split(line)?)
}

#[cfg(any(target_os = "macos", test))]
pub fn launchd_job(text: &str) -> Option<Option<PathBuf>> {
    let (_, after) = text.split_once("<key>ProgramArguments</key>")?;
    let (array, _) = after.split_once("</array>")?;
    let args: Vec<String> = array
        .split("<string>")
        .skip(1)
        .filter_map(|s| s.split_once("</string>").map(|(v, _)| unxml(v)))
        .collect();
    burrow_command(&args)
}

#[cfg(any(target_os = "macos", test))]
fn unxml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub fn read_units(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    parse: fn(&str) -> Option<Option<PathBuf>>,
) -> Vec<Unit> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    listing
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().to_str()?.to_string();
            let name = file.strip_prefix(prefix)?.strip_suffix(suffix)?;
            if name != "burrow" && !name.starts_with("burrow-") {
                return None;
            }
            if !entry.path().is_file() {
                return None;
            }
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let config = parse(&text)?;
            Some(Unit {
                name: name.to_string(),
                label: file.strip_suffix(".plist").unwrap_or(&file).to_string(),
                config,
            })
        })
        .collect()
}

pub fn bare(name: &str) -> Manifest {
    Manifest {
        name: name.to_string(),
        role: String::new(),
        config: PathBuf::new(),
        source_config: PathBuf::new(),
        env_file: None,
        log_file: PathBuf::new(),
        binary: PathBuf::new(),
        copied_binary: false,
        account: String::new(),
        created_account: false,
        low_ports: false,
        files: Vec::new(),
        version: String::new(),
        admin_writable: false,
    }
}

fn merge(
    known: &[Manifest],
    records: Vec<(String, Option<PathBuf>)>,
    units: Vec<Unit>,
) -> Vec<Unit> {
    let mut found: Vec<Unit> = units;
    for (name, config) in records {
        match found.iter_mut().find(|u| u.name == name) {
            Some(u) => {
                if u.config.is_none() {
                    u.config = config;
                }
            }
            None => found.push(Unit {
                label: name.clone(),
                name,
                config,
            }),
        }
    }
    found.retain(|u| {
        (u.name == "burrow" || u.name.starts_with("burrow-"))
            && !known.iter().any(|m| m.name == u.name)
    });
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct OldRecord {
    config: Option<PathBuf>,
    env_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
}

fn records(dir: &Path) -> Vec<(String, OldRecord)> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| read_manifest(p).is_err())
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            let record = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<OldRecord>(&t).ok())
                .unwrap_or_default();
            Some((name, record))
        })
        .collect()
}

fn unusable_records(dir: &Path) -> Vec<(String, Option<PathBuf>)> {
    records(dir)
        .into_iter()
        .map(|(name, r)| (name, r.config))
        .collect()
}

pub fn leftovers_in(dir: &Path, name: Option<&str>) -> Vec<PathBuf> {
    records(dir)
        .into_iter()
        .filter(|(n, _)| name.is_none_or(|x| x == n))
        .flat_map(|(_, r)| {
            let logs = r
                .log_file
                .filter(|l| !l.as_os_str().is_empty())
                .into_iter()
                .flat_map(|l| {
                    let mut all: Vec<PathBuf> = (1..=crate::base::logfile::KEEP)
                        .map(|n| PathBuf::from(format!("{}.{n}", l.display())))
                        .collect();
                    all.push(l);
                    all
                });
            r.config.into_iter().chain(r.env_file).chain(logs)
        })
        .collect()
}

pub fn leftovers(name: Option<&str>) -> Vec<PathBuf> {
    leftovers_in(&crate::install::Platform::current().state_dir(), name)
}

pub fn strays() -> Vec<Stray> {
    let Ok(backend) = system::backend() else {
        return Vec::new();
    };
    let records = unusable_records(&crate::install::Platform::current().state_dir());
    merge(&all_manifests(), records, backend.units())
        .into_iter()
        .filter_map(|u| {
            let st = backend.status(&bare(&u.name));
            st.installed.then(|| Stray {
                stop: backend.manual_stop(&u.name),
                name: u.name,
                label: u.label,
                config: u.config,
                running: st.running,
                pid: st.pid,
            })
        })
        .collect()
}

pub fn named(name: &str) -> Option<Stray> {
    strays().into_iter().find(|s| s.name == name)
}

impl Stray {
    fn state(&self) -> &'static str {
        if self.running { "running" } else { "stopped" }
    }

    fn install(&self) -> String {
        let config = self
            .config
            .as_deref()
            .map_or_else(|| "CONFIG".to_string(), crate::base::failure::path_arg);
        let named = self
            .config
            .as_deref()
            .and_then(|c| service_name(c, None).ok())
            .is_some_and(|n| n == self.name);
        let name = if named {
            String::new()
        } else {
            format!(
                " --name {}",
                self.name.strip_prefix("burrow-").unwrap_or(&self.name)
            )
        };
        crate::base::failure::admin(&format!("service install -c {config}{name}"))
    }

    fn steps(&self) -> String {
        if self.running {
            format!("{}, then {}", self.stop, self.install())
        } else {
            self.install()
        }
    }

    pub fn failure(&self) -> Failure {
        Failure::new(format!(
            "{} is {}, but burrow has no usable record of it (installed by an older burrow or by hand)",
            self.label,
            self.state()
        ))
        .hint(format!("take it over: {}", self.steps()))
    }

    pub fn line(&self) -> String {
        format!(
            "{:<24} {:<8} {:<8} not managed by this burrow (older layout or hand-made); see: {}",
            self.name,
            "?",
            self.state(),
            crate::base::failure::cmd(&format!("service status {}", self.name))
        )
    }

    pub fn note(&self, out: &mut dyn Write) -> Result<(), Failure> {
        say(
            out,
            format!(
                "note: {} ({}) was not restarted, because burrow has no usable record of it; take it over: {}",
                self.label,
                self.state(),
                self.steps()
            ),
        )
    }
}

pub fn none_usable(strays: &[Stray]) -> Failure {
    match strays {
        [] => Failure::new("no burrow service is installed").hint(format!(
            "install one with: {}",
            crate::base::failure::admin("service install")
        )),
        [one] => one.failure(),
        many => Failure::new(format!(
            "{} are installed, but burrow has no usable record of them (installed by an older burrow or by hand)",
            many.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(", ")
        ))
        .hint(format!(
            "see how to take each over with: {}",
            crate::base::failure::cmd(&format!("service status {}", many[0].name))
        )),
    }
}

pub fn keeps(strays: &[Stray], what: impl std::fmt::Display) -> Option<String> {
    let labels: Vec<&str> = strays.iter().map(|s| s.label.as_str()).collect();
    match labels.as_slice() {
        [] => None,
        [one] => Some(format!("kept {what}: {one} still uses it")),
        many => Some(format!("kept {what}: {} still use it", many.join(", "))),
    }
}

pub fn pick(all: Vec<Manifest>, strays: &[Stray]) -> Result<Manifest, Failure> {
    match (all.as_slice(), strays) {
        ([one], []) => Ok(one.clone()),
        ([], _) => Err(none_usable(strays)),
        _ => Err(
            Failure::usage("more than one burrow service is installed; say which one").hint(
                format!(
                    "installed: {}",
                    all.iter()
                        .map(|m| m.name.clone())
                        .chain(
                            strays
                                .iter()
                                .map(|s| format!("{} (not managed by this burrow)", s.name))
                        )
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_a_burrow_binary_running_a_role_is_a_burrow_command() {
        let config = Some(PathBuf::from("/etc/burrow/client.yaml"));
        for line in [
            &[
                "/usr/local/bin/burrow",
                "client",
                "-c",
                "/etc/burrow/client.yaml",
            ][..],
            &[
                "/usr/local/bin/burrow",
                "client",
                "--config",
                "/etc/burrow/client.yaml",
            ],
            &[
                "/usr/local/bin/burrow",
                "client",
                "--config=/etc/burrow/client.yaml",
            ],
            &[
                r"C:\Program Files\burrow\burrow.exe",
                "client",
                "-c",
                "/etc/burrow/client.yaml",
            ],
        ] {
            assert_eq!(
                burrow_command(&args(line)),
                Some(config.clone()),
                "{line:?}"
            );
        }
        assert_eq!(
            burrow_command(&args(&["/usr/local/bin/burrow", "visit"])),
            Some(None)
        );
        for line in [
            &["/usr/local/bin/burrow", "admin"][..],
            &["/usr/local/bin/burrowd", "client", "-c", "x.yaml"],
            &["/usr/bin/python3", "/opt/burrow", "client"],
            &["/usr/local/bin/burrow"],
            &[],
        ] {
            assert_eq!(burrow_command(&args(line)), None, "{line:?}");
        }
    }

    const OLD_UNIT: &str = "[Unit]\nDescription=burrow client \"client\"\n[Service]\nUser=burrow\nExecStart=/usr/local/bin/burrow client -c /etc/burrow/client.yaml --log-file /var/log/burrow/client/client.log\nExecReload=/bin/kill -HUP $MAINPID\n";

    #[test]
    fn a_systemd_unit_counts_only_when_it_runs_burrow() {
        assert_eq!(
            systemd_unit(OLD_UNIT),
            Some(Some(PathBuf::from("/etc/burrow/client.yaml")))
        );
        assert_eq!(
            systemd_unit(
                "[Service]\nExecStart=-\"/opt/my tools/burrow\" server -c \"/etc/my burrow/s.yaml\"\n"
            ),
            Some(Some(PathBuf::from("/etc/my burrow/s.yaml")))
        );
        assert_eq!(
            systemd_unit("[Service]\nExecStart=/usr/bin/burrow-exporter --port 9100\n"),
            None
        );
        assert_eq!(
            systemd_unit("[Service]\nExecStart=/usr/bin/burrow status\n"),
            None
        );
        assert_eq!(systemd_unit("[Unit]\nDescription=burrow\n"), None);
    }

    #[test]
    fn a_launchd_job_counts_only_when_it_runs_burrow() {
        let job = |program: &str| {
            format!(
                "<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>com.github.charlesng35.burrow-home</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{program}</string>\n    <string>client</string>\n    <string>-c</string>\n    <string>/etc/burrow/home &amp; more.yaml</string>\n  </array>\n</dict>\n</plist>\n"
            )
        };
        assert_eq!(
            launchd_job(&job("/usr/local/bin/burrow")),
            Some(Some(PathBuf::from("/etc/burrow/home & more.yaml")))
        );
        assert_eq!(launchd_job(&job("/usr/local/bin/other")), None);
        assert_eq!(launchd_job("<plist></plist>"), None);
    }

    #[test]
    fn units_are_read_from_their_folder_and_only_burrow_s_own_count() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        write("burrow-client.service", OLD_UNIT);
        write("burrow.service", OLD_UNIT);
        write(
            "burrow-exporter.service",
            "[Service]\nExecStart=/usr/bin/exporter\n",
        );
        write("nginx.service", OLD_UNIT);
        write("burrow-client.service.bak", OLD_UNIT);
        std::fs::create_dir(dir.path().join("burrow-x.service")).unwrap();
        let mut found = read_units(dir.path(), "", ".service", systemd_unit);
        found.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<(&str, &str)> = found
            .iter()
            .map(|u| (u.name.as_str(), u.label.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("burrow", "burrow.service"),
                ("burrow-client", "burrow-client.service")
            ]
        );
        assert_eq!(
            found[1].config.as_deref(),
            Some(Path::new("/etc/burrow/client.yaml"))
        );
        let jobs = read_units(dir.path(), "com.github.charlesng35.", ".plist", launchd_job);
        assert!(jobs.is_empty());
        assert!(read_units(&dir.path().join("missing"), "", ".service", systemd_unit).is_empty());
    }

    fn unit(name: &str, config: Option<&str>) -> Unit {
        Unit {
            name: name.into(),
            label: format!("{name}.service"),
            config: config.map(PathBuf::from),
        }
    }

    #[test]
    fn strays_are_the_units_and_records_burrow_cannot_use() {
        let known = [bare("burrow-server")];
        let records = vec![
            (
                "burrow-client".to_string(),
                Some(PathBuf::from("/old/client.yaml")),
            ),
            ("burrow-gone".to_string(), None),
            ("not-burrow".to_string(), None),
        ];
        let units = vec![
            unit("burrow-server", Some("/etc/burrow/server/server.yaml")),
            unit("burrow-client", Some("/etc/burrow/client.yaml")),
            unit("burrow-hand", None),
        ];
        let found = merge(&known, records, units);
        let names: Vec<(&str, &str, Option<&Path>)> = found
            .iter()
            .map(|u| (u.name.as_str(), u.label.as_str(), u.config.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "burrow-client",
                    "burrow-client.service",
                    Some(Path::new("/etc/burrow/client.yaml"))
                ),
                ("burrow-gone", "burrow-gone", None),
                ("burrow-hand", "burrow-hand.service", None),
            ]
        );
    }

    fn stray(name: &str, running: bool, config: Option<&str>) -> Stray {
        Stray {
            name: name.into(),
            label: format!("{name}.service"),
            config: config.map(PathBuf::from),
            running,
            pid: running.then_some(42),
            stop: format!("sudo systemctl stop {name}"),
        }
    }

    #[test]
    fn a_stray_names_the_exact_commands_that_take_it_over() {
        let f = stray("burrow-client", true, Some("/etc/burrow/client.yaml")).failure();
        assert_eq!(
            f.message,
            "burrow-client.service is running, but burrow has no usable record of it (installed by an older burrow or by hand)"
        );
        let admin = crate::base::failure::admin;
        assert_eq!(
            f.hint.unwrap(),
            format!(
                "take it over: sudo systemctl stop burrow-client, then {}",
                admin("service install -c /etc/burrow/client.yaml")
            )
        );
        let f = stray("burrow-home", false, Some("/etc/burrow/home/client.yaml")).failure();
        assert!(
            f.message.starts_with("burrow-home.service is stopped, but"),
            "{}",
            f.message
        );
        let hint = f.hint.unwrap();
        assert!(!hint.contains("systemctl"), "{hint}");
        assert!(
            hint.ends_with(&admin(
                "service install -c /etc/burrow/home/client.yaml --name home"
            )),
            "{hint}"
        );
        let hint = stray("burrow-hand", false, None).failure().hint.unwrap();
        assert!(
            hint.ends_with(&admin("service install -c CONFIG --name hand")),
            "{hint}"
        );
    }

    fn manifest(name: &str) -> Manifest {
        bare(name)
    }

    #[test]
    fn a_stray_is_counted_when_a_service_is_picked_without_a_name() {
        let one = manifest("burrow-server");
        assert_eq!(pick(vec![one.clone()], &[]).unwrap().name, "burrow-server");
        let f = pick(vec![], &[]).unwrap_err();
        assert_eq!(f.message, "no burrow service is installed");
        let s = stray("burrow-client", true, Some("/etc/burrow/client.yaml"));
        let f = pick(vec![], std::slice::from_ref(&s)).unwrap_err();
        assert_eq!(f, s.failure());
        let other = stray("burrow-home", false, None);
        let f = pick(vec![], &[s.clone(), other.clone()]).unwrap_err();
        assert_eq!(
            f.message,
            "burrow-client.service, burrow-home.service are installed, but burrow has no usable record of them (installed by an older burrow or by hand)"
        );
        assert_eq!(none_usable(&[s.clone(), other]), f);
        let f = pick(vec![one], std::slice::from_ref(&s)).unwrap_err();
        assert!(f.message.contains("more than one"), "{}", f.message);
        assert_eq!(
            f.hint.unwrap(),
            "installed: burrow-server, burrow-client (not managed by this burrow)"
        );
    }

    #[test]
    fn an_update_notes_a_stray_it_left_alone_once() {
        let mut out = Vec::new();
        stray("burrow-client", true, Some("/etc/burrow/client.yaml"))
            .note(&mut out)
            .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.starts_with(&format!(
                "note: burrow-client.service (running) was not restarted, because burrow has no usable record of it; take it over: sudo systemctl stop burrow-client, then {}",
                crate::base::failure::admin("service install -c /etc/burrow/client.yaml")
            )),
            "{out}"
        );
        assert_eq!(out.matches("burrow-client.service").count(), 1, "{out}");
    }

    #[test]
    fn what_a_stray_may_still_use_is_kept_and_named() {
        let bin = Path::new("/usr/local/bin/burrow");
        assert_eq!(keeps(&[], bin.display()), None);
        assert_eq!(
            keeps(&[stray("burrow-stray", true, None)], bin.display()).as_deref(),
            Some("kept /usr/local/bin/burrow: burrow-stray.service still uses it")
        );
        assert_eq!(
            keeps(
                &[
                    stray("burrow-a", false, None),
                    stray("burrow-b", true, None)
                ],
                "service account burrow"
            )
            .as_deref(),
            Some("kept service account burrow: burrow-a.service, burrow-b.service still use it")
        );
    }

    #[test]
    fn the_files_an_unusable_record_names_are_its_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("burrow-old.json"),
            r#"{"name":"burrow-old","config":"/etc/burrow/old.yaml","env_file":"/etc/burrow/old.env","log_file":"/var/log/burrow/old/old.log"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("burrow-other.json"),
            r#"{"name":"burrow-other","config":"/etc/burrow/other.yaml","env_file":null,"log_file":""}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("burrow-junk.json"), "not json").unwrap();
        let mut old: Vec<PathBuf> = ["/etc/burrow/old.yaml", "/etc/burrow/old.env"]
            .into_iter()
            .map(PathBuf::from)
            .chain(
                std::iter::once("/var/log/burrow/old/old.log".to_string())
                    .chain(
                        (1..=crate::base::logfile::KEEP)
                            .map(|n| format!("/var/log/burrow/old/old.log.{n}")),
                    )
                    .map(PathBuf::from),
            )
            .collect();
        old.sort();
        let mut found = leftovers_in(dir.path(), Some("burrow-old"));
        found.sort();
        assert_eq!(found, old);
        let mut all = leftovers_in(dir.path(), None);
        all.sort();
        old.push(PathBuf::from("/etc/burrow/other.yaml"));
        old.sort();
        assert_eq!(all, old);
        assert!(leftovers_in(&dir.path().join("missing"), None).is_empty());
    }

    #[test]
    fn a_stray_in_the_list_says_how_to_learn_more() {
        let line = stray("burrow-client", true, None).line();
        assert!(line.starts_with("burrow-client"), "{line}");
        assert!(
            line.contains("running  not managed by this burrow (older layout or hand-made); see: "),
            "{line}"
        );
        assert!(line.ends_with("service status burrow-client"), "{line}");
    }
}
