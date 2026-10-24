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
        web_state: None,
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
mod tests;
