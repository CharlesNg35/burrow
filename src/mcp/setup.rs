use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::http::ENDPOINT;
use super::mode::Writes;
use crate::admin_link::token::Token;
use crate::base::failure::{arg, found_on_path, program_arg};

pub const SERVER_NAME: &str = "burrow";
pub const TOKEN_VAR: &str = "BURROW_MCP_TOKEN";
pub const ALLOW_WRITE: &str = "--allow-write";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Claude,
    Codex,
    Cursor,
}

pub const APPS: [(App, &str, &str); 3] = [
    (App::Claude, "claude", "Claude Code"),
    (App::Codex, "codex", "Codex"),
    (App::Cursor, "cursor", "Cursor"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

pub struct Printed {
    pub head: String,
    pub head_to: Stream,
    pub text: String,
    pub after: Option<String>,
}

#[derive(Serialize)]
pub struct Launch {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Serialize)]
pub struct Remote {
    pub url: String,
    pub headers: BTreeMap<&'static str, String>,
}

pub enum Setup {
    Stdio(Launch),
    Http { remote: Remote, token: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile<T> {
    mcp_servers: BTreeMap<&'static str, T>,
}

pub const BEFORE: &str = "Add burrow to your AI app with this standard MCP config (Claude Desktop, Cursor, Cherry Studio's JSON import and most apps take it as is):";

fn json_file<T: Serialize>(entry: &T) -> String {
    let file = ConfigFile {
        mcp_servers: BTreeMap::from([(SERVER_NAME, entry)]),
    };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn export(token: &str) -> String {
    if cfg!(windows) {
        format!("then, in the PowerShell that starts Codex:\n  $env:{TOKEN_VAR} = \"{token}\"")
    } else {
        format!("then, in the shell that starts Codex:\n  export {TOKEN_VAR}={token}")
    }
}

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{ENDPOINT}")
}

pub fn program() -> PathBuf {
    match std::env::current_exe().and_then(dunce::canonicalize) {
        Ok(path) => found_on_path(&path).unwrap_or(path),
        Err(e) => {
            crate::say_errln!(
                "cannot find where this burrow is ({e}), so the setup names plain `burrow`, which must then be on PATH"
            );
            PathBuf::from("burrow")
        }
    }
}

pub fn absolute(config: &Path) -> PathBuf {
    dunce::canonicalize(config)
        .or_else(|_| std::path::absolute(config))
        .unwrap_or_else(|_| config.to_path_buf())
}

pub fn args(config: &Path, writes: Writes) -> Vec<String> {
    let mut args = vec![
        "mcp".to_string(),
        "-c".to_string(),
        absolute(config).display().to_string(),
    ];
    if writes == Writes::Allowed {
        args.push(ALLOW_WRITE.to_string());
    }
    args
}

impl Setup {
    pub fn stdio(program: &Path, config: &Path, writes: Writes) -> Setup {
        Setup::Stdio(Launch {
            command: program.display().to_string(),
            args: args(config, writes),
        })
    }

    pub fn http(port: u16, token: &Token) -> Setup {
        Setup::Http {
            remote: Remote {
                url: url(port),
                headers: BTreeMap::from([("Authorization", format!("Bearer {}", token.as_str()))]),
            },
            token: token.as_str().to_string(),
        }
    }

    pub fn json(&self) -> String {
        match self {
            Setup::Stdio(l) => json_file(l),
            Setup::Http { remote, .. } => json_file(remote),
        }
    }

    pub fn form(&self) -> String {
        match self {
            Setup::Stdio(l) => {
                let mut form = format!(
                    "In an app with a form, like Cherry Studio, put each argument on its own line:\nCommand:\n  {}\nArguments, one per line:\n",
                    l.command
                );
                for a in &l.args {
                    form.push_str(&format!("  {a}\n"));
                }
                form
            }
            Setup::Http { remote, token } => format!(
                "In an app with a form, choose the HTTP (streamable HTTP) type:\nURL:\n  {}\nHeader:\n  Authorization: Bearer {token}\n",
                remote.url
            ),
        }
    }

    pub fn printed(&self, app: Option<App>) -> Printed {
        let Some(app) = app else {
            return Printed {
                head: BEFORE.into(),
                head_to: Stream::Stderr,
                text: self.json(),
                after: Some(self.form()),
            };
        };
        let (head, head_to) = match app {
            App::Claude => ("# Claude Code: run once in a terminal", Stream::Stdout),
            App::Codex => ("# ~/.codex/config.toml", Stream::Stdout),
            App::Cursor => ("// ~/.cursor/mcp.json", Stream::Stderr),
        };
        Printed {
            head: head.into(),
            head_to,
            text: self.for_app(app),
            after: match (app, self) {
                (App::Codex, Setup::Http { token, .. }) => Some(export(token)),
                _ => None,
            },
        }
    }

    fn for_app(&self, app: App) -> String {
        match (app, self) {
            (App::Cursor, _) => self.json(),
            (App::Claude, Setup::Stdio(l)) => {
                let mut words = vec![program_arg(Path::new(&l.command))];
                words.extend(l.args.iter().map(|a| arg(a)));
                format!("claude mcp add {SERVER_NAME} -- {}", words.join(" "))
            }
            (App::Claude, Setup::Http { remote, token }) => format!(
                "claude mcp add --transport http {SERVER_NAME} {} --header \"Authorization: Bearer {token}\"",
                remote.url
            ),
            (App::Codex, Setup::Stdio(l)) => format!(
                "[mcp_servers.{SERVER_NAME}]\ncommand = {}\nargs = [{}]",
                toml_str(&l.command),
                l.args
                    .iter()
                    .map(|a| toml_str(a))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            (App::Codex, Setup::Http { remote, .. }) => format!(
                "[mcp_servers.{SERVER_NAME}]\nurl = {}\nbearer_token_env_var = \"{TOKEN_VAR}\"",
                toml_str(&remote.url)
            ),
        }
    }
}

pub fn hints(program: &Path, config: &Path, writes: Writes, http: Option<u16>) -> String {
    let mut words = vec![program_arg(program)];
    words.extend(args(config, writes).iter().map(|a| arg(a)));
    match http {
        None => {}
        Some(port) if port == burrow_syntax::ports::MCP_PORT => words.push("--http".into()),
        Some(port) => words.extend(["--http".into(), "--port".into(), port.to_string()]),
    }
    let base = words.join(" ");
    let apps: Vec<&str> = APPS.iter().map(|(_, word, _)| *word).collect();
    let mut lines = vec![
        ("Any app:", format!("{base} --print-config")),
        (
            "One app:",
            format!("{base} --print-config {}", apps.join("|")),
        ),
    ];
    if http.is_none() {
        lines.push(("Check it:", format!("{base} --check")));
    }
    let mut out = String::from("Setup for your AI app:\n");
    for (label, line) in lines {
        out.push_str(&format!("  {label:<10} {line}\n"));
    }
    out
}

pub fn started(port: u16, token: Option<&Token>, hints: &str) -> String {
    let mut out = String::from("burrow mcp: MCP over HTTP\n");
    out.push_str(&format!("  URL:    {}\n", url(port)));
    let Some(token) = token else {
        return out;
    };
    out.push_str(&format!(
        "  Header: Authorization: Bearer {}\n\n{hints}",
        token.as_str()
    ));
    out
}

pub fn by_hand(hints: &str) -> String {
    format!(
        "burrow mcp speaks MCP on stdin and stdout and is meant to be started by your AI app; stop it with Ctrl-C\n\n{hints}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: &str = "/opt/my burrow/burrow";

    fn token() -> Token {
        Token::parse(&"ab".repeat(32)).unwrap()
    }

    fn launch_of(s: &Setup) -> (String, Vec<String>) {
        let v: serde_json::Value = serde_json::from_str(&s.json()).unwrap();
        let e = &v["mcpServers"]["burrow"];
        (
            e["command"].as_str().unwrap().to_string(),
            serde_json::from_value(e["args"].clone()).unwrap(),
        )
    }

    #[test]
    fn the_standard_stdio_config_is_json_and_a_form_with_one_argument_per_line() {
        let config = Path::new("/home/me/ops \"x\".yaml");
        for writes in [Writes::Off, Writes::Allowed] {
            let s = Setup::stdio(Path::new(PROGRAM), config, writes);
            let (command, args) = launch_of(&s);
            assert_eq!(command, PROGRAM);
            assert_eq!(args[..2], ["mcp", "-c"]);
            assert_eq!(args.len(), 3 + usize::from(writes == Writes::Allowed));
            let form = s.form();
            let lines: Vec<&str> = form.lines().collect();
            assert_eq!(lines[1..3], ["Command:", "  /opt/my burrow/burrow"]);
            assert_eq!(lines[3], "Arguments, one per line:");
            let listed: Vec<&str> = lines[4..].iter().map(|l| l.trim()).collect();
            assert_eq!(listed, args, "{form}");
            let p = s.printed(None);
            assert_eq!((p.head.as_str(), p.head_to), (BEFORE, Stream::Stderr));
            assert_eq!(p.text, s.json());
            assert_eq!(p.after, Some(form));
        }
    }

    #[test]
    fn each_app_format_wraps_the_same_command_and_arguments() {
        let config = Path::new("/home/me/ops \"x\".yaml");
        let s = Setup::stdio(Path::new(PROGRAM), config, Writes::Allowed);
        let (command, args) = launch_of(&s);
        let mut whole = vec![command.clone()];
        whole.extend(args.iter().cloned());
        let codex: toml::Value = toml::from_str(&s.printed(Some(App::Codex)).text).unwrap();
        let entry = &codex["mcp_servers"]["burrow"];
        assert_eq!(entry["command"].as_str(), Some(command.as_str()));
        let toml_args: Vec<String> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        assert_eq!(toml_args, args);
        assert_eq!(s.printed(Some(App::Cursor)).text, s.json());
        let claude = s.printed(Some(App::Claude));
        assert_eq!(claude.head_to, Stream::Stdout);
        if !cfg!(windows) {
            let words = shlex::split(&claude.text).unwrap();
            assert_eq!(words[..5], ["claude", "mcp", "add", "burrow", "--"]);
            assert_eq!(words[5..], whole);
        }
        for (app, _, _) in APPS {
            let p = s.printed(Some(app));
            assert!(p.head.starts_with("# ") || p.head.starts_with("// "));
        }
    }

    #[test]
    fn the_http_config_and_its_app_formats_share_the_url_and_token() {
        let s = Setup::http(7838, &token());
        let v: serde_json::Value = serde_json::from_str(&s.json()).unwrap();
        let entry = &v["mcpServers"]["burrow"];
        assert_eq!(entry["url"], "http://127.0.0.1:7838/mcp");
        let bearer = format!("Bearer {}", "ab".repeat(32));
        assert_eq!(entry["headers"]["Authorization"], bearer.as_str());
        assert!(s.form().contains("URL:\n  http://127.0.0.1:7838/mcp\n"));
        let claude = s.printed(Some(App::Claude)).text;
        assert!(claude.contains(&bearer) && claude.contains("http://127.0.0.1:7838/mcp"));
        let codex = s.printed(Some(App::Codex));
        let t: toml::Value = toml::from_str(&codex.text).unwrap();
        assert_eq!(
            t["mcp_servers"]["burrow"]["bearer_token_env_var"].as_str(),
            Some(TOKEN_VAR)
        );
        assert!(codex.after.unwrap().contains(&"ab".repeat(32)));
        assert_eq!(s.printed(Some(App::Cursor)).text, s.json());
    }

    #[test]
    fn the_hints_name_the_full_command_for_any_app_one_app_and_the_check() {
        let config = Path::new("/srv/ops laptop.yaml");
        let h = hints(Path::new(PROGRAM), config, Writes::Allowed, None);
        let shown = absolute(config).display().to_string();
        let c = if cfg!(windows) {
            format!("& \"/opt/my burrow/burrow\" mcp -c \"{shown}\" --allow-write")
        } else {
            format!("'/opt/my burrow/burrow' mcp -c '{shown}' --allow-write")
        };
        assert_eq!(
            h,
            format!(
                "Setup for your AI app:\n  Any app:   {c} --print-config\n  One app:   {c} --print-config claude|codex|cursor\n  Check it:  {c} --check\n"
            )
        );
        assert_eq!(
            by_hand(&h),
            format!(
                "burrow mcp speaks MCP on stdin and stdout and is meant to be started by your AI app; stop it with Ctrl-C\n\n{h}"
            )
        );
        let web = hints(
            Path::new("/usr/bin/burrow"),
            config,
            Writes::Off,
            Some(7838),
        );
        let lines: Vec<&str> = web.lines().collect();
        assert_eq!(lines.len(), 3, "no --check over HTTP: {web}");
        assert!(lines[1].ends_with("--http --print-config"), "{web}");
        let other = hints(
            Path::new("/usr/bin/burrow"),
            config,
            Writes::Off,
            Some(9000),
        );
        assert!(
            other.contains("--http --port 9000 --print-config"),
            "{other}"
        );
        let text = started(7838, Some(&token()), &web);
        assert!(text.starts_with(
            "burrow mcp: MCP over HTTP\n  URL:    http://127.0.0.1:7838/mcp\n  Header: Authorization: Bearer "
        ));
        assert!(text.ends_with(&format!("\n\n{web}")));
        assert_eq!(
            started(7838, None, &web),
            "burrow mcp: MCP over HTTP\n  URL:    http://127.0.0.1:7838/mcp\n"
        );
    }

    #[test]
    fn the_program_is_this_burrow_as_an_absolute_path() {
        let program = program();
        assert!(program.is_absolute(), "{}", program.display());
        assert_eq!(
            dunce::canonicalize(&program).unwrap(),
            dunce::canonicalize(std::env::current_exe().unwrap()).unwrap()
        );
    }
}
