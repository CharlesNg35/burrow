mod merge;
mod parse;

use std::path::Path;

use super::env::EnvSource;
use super::settings::{ClientSettings, ServerSettings, VisitorSettings};
use super::tree::{self, Node, Pos, Span, Value};
use crate::config::diagnostics::{Diagnostic, Report};
use burrow_config::{ClientConfig, ServerConfig, VisitorConfig};
pub use merge::merge;
pub use parse::{load_inputs, load_str_at, load_str_in};

pub use crate::install::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Json,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => Some(Format::Yaml),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub text: String,
    pub format: Format,
}

impl Input {
    pub fn body(&self) -> &str {
        self.text.strip_prefix('\u{feff}').unwrap_or(&self.text)
    }
}

#[derive(Debug, Clone)]
pub enum Loaded {
    Server(Box<ServerSettings>, Box<ServerConfig>),
    Client(Box<ClientSettings>, Box<ClientConfig>),
    Visitor(Box<VisitorSettings>, Box<VisitorConfig>),
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub report: Report,
    pub role: Option<Role>,
    pub loaded: Option<Loaded>,
}

#[derive(Debug)]
pub enum LoadError {
    Read(std::io::Error),
    UnknownExtension,
}

impl LoadError {
    pub fn describe(&self, path: &Path) -> (String, String) {
        match self {
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::NotFound => (
                format!("config file {} does not exist", path.display()),
                format!(
                    "check the path, or create one with: {0} init server  (or: {0} init client, {0} init visitor)",
                    crate::base::failure::command()
                ),
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::PermissionDenied => (
                format!("cannot read {}: permission denied", path.display()),
                if cfg!(windows) {
                    format!(
                        "run as an administrator, or check its permissions with: icacls {}",
                        crate::base::failure::path_arg(path)
                    )
                } else {
                    format!(
                        "run as the file's owner, or check its permissions with: ls -l {}",
                        crate::base::failure::path_arg(path)
                    )
                },
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::InvalidData => (
                format!("{} is not valid UTF-8 text", path.display()),
                "save the file as UTF-8".into(),
            ),
            LoadError::Read(e) => (
                format!(
                    "cannot read {}: {}",
                    path.display(),
                    crate::base::failure::file_reason(e)
                ),
                "check that the path is a readable file".into(),
            ),
            LoadError::UnknownExtension => (
                format!("cannot tell the format of {}", path.display()),
                "name the file with a .yaml, .yml or .json extension".into(),
            ),
        }
    }
}

pub fn load_file(
    path: &Path,
    role: Option<Role>,
    env: &dyn EnvSource,
) -> Result<Outcome, LoadError> {
    let format = Format::from_path(path).ok_or(LoadError::UnknownExtension)?;
    let meta = std::fs::metadata(path).map_err(LoadError::Read)?;
    if !meta.is_file() && !meta.is_dir() {
        return Err(LoadError::Read(std::io::Error::other(
            "it is not a regular file",
        )));
    }
    let src = std::fs::read_to_string(path).map_err(LoadError::Read)?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let named = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().ends_with("visitor"));
    let role = role.or_else(|| named.then(|| visitor_by_name(&src)).flatten());
    let state = crate::install::state_dir_for(path);
    let mut outcome = load_str_at(
        (base, Some(&state)),
        &path.display().to_string(),
        &src,
        format,
        role,
        env,
    );
    if let Some(Loaded::Server(s, _)) = outcome.loaded.as_mut()
        && let Some(w) = s.web.as_mut()
    {
        w.history = Some(crate::config::save::history_dir_of(path));
    }
    if let Some(w) = permission_warning(path) {
        outcome.report.push(w);
        outcome.report.sort();
    }
    if outcome
        .report
        .diagnostics
        .iter()
        .any(|d| d.code == burrow_admin_proto::codes::SYNTAX)
        && let Some(spare) = crate::config::save::spare_next_to(path)
    {
        outcome.report.push(
            Diagnostic::warning(
                "edit-spare",
                format!(
                    "{} holds the text of a burrow edit save that did not finish",
                    spare.display()
                ),
            )
            .help("if this file was cut short while saving, copy that file over it"),
        );
        outcome.report.sort();
    }
    Ok(outcome)
}

#[cfg(unix)]
fn permission_warning(path: &Path) -> Option<Diagnostic> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    (mode & 0o004 != 0).then(|| {
        Diagnostic::warning(
            "world-readable",
            format!(
                "config file is readable by every user on this machine (mode {:o})",
                mode & 0o777
            ),
        )
        .help(format!(
            "restrict it with: chmod 600 {}",
            crate::base::failure::path_arg(path)
        ))
    })
}

#[cfg(not(unix))]
fn permission_warning(_: &Path) -> Option<Diagnostic> {
    None
}

fn visitor_by_name(src: &str) -> Option<Role> {
    let root = tree::parse(src).ok()?;
    let says_client = match &root.value {
        Value::Map(entries) => entries
            .iter()
            .any(|e| e.key == "allow" || e.key == "allow_all"),
        _ => false,
    };
    (detect_role(&root) != Some(Role::Server) && !says_client).then_some(Role::Visitor)
}

pub fn detect_role(root: &Node) -> Option<Role> {
    let Value::Map(entries) = &root.value else {
        return None;
    };
    if entries
        .iter()
        .any(|e| e.key == "forwards" || e.key == "forward")
    {
        return Some(Role::Visitor);
    }
    let server = entries
        .iter()
        .filter(|e| {
            use burrow_config::ServerConfig as S;
            [
                S::LISTEN.key(),
                S::WEBSOCKET.key(),
                S::CLIENTS.key(),
                S::VISITORS.key(),
                S::SERVICES.key(),
                S::LIMITS.key(),
                S::POOL.key(),
                S::HEARTBEAT.key(),
            ]
            .contains(&e.key.as_str())
        })
        .count();
    let client = entries
        .iter()
        .filter(|e| {
            matches!(
                e.key.as_str(),
                "server"
                    | "name"
                    | "secret"
                    | "allow"
                    | "allow_all"
                    | "proxy"
                    | "reconnect"
                    | "cipher"
            )
        })
        .count();
    match server.cmp(&client) {
        std::cmp::Ordering::Greater => Some(Role::Server),
        std::cmp::Ordering::Less => Some(Role::Client),
        std::cmp::Ordering::Equal => None,
    }
}

pub fn schema_json(role: Role) -> serde_json::Value {
    let schema = match role {
        Role::Server => schemars::schema_for!(ServerConfig),
        Role::Client => schemars::schema_for!(ClientConfig),
        Role::Visitor => schemars::schema_for!(VisitorConfig),
    };
    serde_json::to_value(schema).expect("schema serializes")
}

fn json_syntax(src: &str) -> Option<Diagnostic> {
    let err = serde_json::from_str::<serde::de::IgnoredAny>(src).err()?;
    let line = err.line();
    let col = err.column().max(1);
    let offset = src
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>()
        + col
        - 1;
    let pos = Pos { offset, line, col };
    let msg = err.to_string();
    let msg = msg.split(" at line ").next().unwrap_or(&msg).to_string();
    Some(
        Diagnostic::error(
            burrow_admin_proto::codes::SYNTAX,
            format!("invalid JSON: {msg}"),
        )
        .at(Span::point(pos))
        .label("here")
        .help("check for a missing comma, quote or bracket just before this point"),
    )
}

fn hide_secret_lines(report: &mut Report) {
    let secret_at = |report: &Report, s: Span| {
        let line = report
            .text_of(s.file)
            .lines()
            .nth(s.start.line.saturating_sub(1))
            .unwrap_or("");
        secret_line(line)
    };
    let hidden: Vec<bool> = report
        .diagnostics
        .iter()
        .map(|d| d.span.is_some_and(|s| secret_at(report, s)))
        .collect();
    for (d, hide) in report.diagnostics.iter_mut().zip(hidden) {
        if hide {
            d.snippet = false;
        }
    }
}

fn secret_line(line: &str) -> bool {
    let key = line.split(':').next().unwrap_or("");
    let key = key.trim_matches(|c: char| c.is_whitespace() || "-\"'{[,".contains(c));
    crate::config::check::secretish(key) || line.contains(burrow_syntax::secret_fmt::SECRET_WORD)
}

pub fn load_str(
    name: &str,
    src: &str,
    format: Format,
    role: Option<Role>,
    env: &dyn EnvSource,
) -> Outcome {
    load_str_in(Path::new("."), name, src, format, role, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_config_names_a_spare_left_by_burrow_edit() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.yaml");
        std::fs::write(&config, "listen: [7835\n").unwrap();
        let nothing = |_: &str| None;
        let codes = |o: &Outcome| {
            o.report
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        };
        let before = load_file(&config, Some(Role::Server), &nothing).unwrap();
        assert!(codes(&before).contains(&burrow_admin_proto::codes::SYNTAX));
        assert!(!codes(&before).contains(&"edit-spare"));
        let spare = dir.path().join(".server.yaml.burrow-edit-9");
        std::fs::write(&spare, "listen: 7835\n").unwrap();
        let after = load_file(&config, Some(Role::Server), &nothing).unwrap();
        let hint = after
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "edit-spare")
            .expect("the spare is named");
        assert!(
            hint.message.contains(".server.yaml.burrow-edit-9"),
            "{}",
            hint.message
        );
        std::fs::write(&config, "listen: 7835\n").unwrap();
        let fine = load_file(&config, Some(Role::Server), &nothing).unwrap();
        assert!(!codes(&fine).contains(&"edit-spare"));
    }

    #[test]
    fn a_problem_in_a_second_source_is_shown_at_its_own_file_and_line() {
        let nothing = |_: &str| None;
        let input = |name: &str, text: &str| Input {
            name: name.into(),
            text: text.into(),
            format: Format::Yaml,
        };
        let o = load_inputs(
            (Path::new("."), None),
            &[
                input(
                    "server.yaml",
                    "listen: 7835\nclients:\n  home: { secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }\n",
                ),
                input(
                    "server.d/apps.yaml",
                    "services:\n  web:\n    client: hom\n    target: 80\n    bind: 7900\n",
                ),
            ],
            Some(Role::Server),
            &nothing,
        );
        let shown = o
            .report
            .render_text(crate::base::style::Style { color: false });
        assert!(shown.contains("--> server.d/apps.yaml:3:13"), "{shown}");
        assert!(shown.contains("client: hom"), "{shown}");
        let json = o.report.render_json();
        assert_eq!(json["file"], "server.yaml");
        assert_eq!(json["diagnostics"][0]["file"], "server.d/apps.yaml");
    }

    #[test]
    fn a_syntax_error_on_a_secret_line_shows_no_snippet() {
        let nothing = |_: &str| None;
        for (src, value) in [
            ("listen: 7835\npassword: hunter2: x\n", "hunter2"),
            ("token: short1: ]\n", "short1"),
            ("listen: 7835\npsk: \"abc1 [\n", "abc1"),
            ("clients:\n  - key: k3y: v\n", "k3y"),
        ] {
            let o = load_str("c.yaml", src, Format::Yaml, Some(Role::Server), &nothing);
            assert!(
                o.report
                    .diagnostics
                    .iter()
                    .any(|d| d.code == burrow_admin_proto::codes::SYNTAX),
                "{src}"
            );
            let shown = o
                .report
                .render_text(crate::base::style::Style { color: false });
            assert!(!shown.contains(value), "{shown}");
        }
    }
}
