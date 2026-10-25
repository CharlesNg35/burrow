use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use serde::Serialize;

use super::http::ENDPOINT;
use crate::admin_link::token::Token;
use crate::base::failure::path_arg;

pub const SERVER_NAME: &str = "burrow";
pub const TOKEN_VAR: &str = "BURROW_MCP_TOKEN";

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

#[derive(Serialize)]
struct Launch {
    command: &'static str,
    args: Vec<String>,
}

#[derive(Serialize)]
struct Remote {
    url: String,
    headers: BTreeMap<&'static str, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorFile<T> {
    mcp_servers: BTreeMap<&'static str, T>,
}

fn cursor<T: Serialize>(entry: T) -> String {
    let file = CursorFile {
        mcp_servers: BTreeMap::from([(SERVER_NAME, entry)]),
    };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

pub struct Setup {
    pub header: String,
    pub text: String,
    pub after: Option<String>,
}

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{ENDPOINT}")
}

pub fn absolute(config: &Path) -> PathBuf {
    dunce::canonicalize(config).unwrap_or_else(|_| config.to_path_buf())
}

fn args(config: &Path) -> Vec<String> {
    vec!["mcp".into(), "-c".into(), config.display().to_string()]
}

fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn export(token: &Token) -> String {
    if cfg!(windows) {
        format!(
            "then, in the PowerShell that starts Codex:\n  $env:{TOKEN_VAR} = \"{}\"",
            token.as_str()
        )
    } else {
        format!(
            "then, in the shell that starts Codex:\n  export {TOKEN_VAR}={}",
            token.as_str()
        )
    }
}

pub fn stdio(app: App, config: &Path) -> Setup {
    let config = absolute(config);
    match app {
        App::Claude => Setup {
            header: "# Claude Code: run once in a terminal".into(),
            text: format!(
                "claude mcp add {SERVER_NAME} -- burrow mcp -c {}",
                path_arg(&config)
            ),
            after: None,
        },
        App::Codex => Setup {
            header: "# ~/.codex/config.toml".into(),
            text: format!(
                "[mcp_servers.{SERVER_NAME}]\ncommand = \"burrow\"\nargs = [{}]",
                args(&config)
                    .iter()
                    .map(|a| toml_str(a))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            after: None,
        },
        App::Cursor => Setup {
            header: "// ~/.cursor/mcp.json".into(),
            text: cursor(Launch {
                command: "burrow",
                args: args(&config),
            }),
            after: None,
        },
    }
}

pub fn http(app: App, port: u16, token: &Token) -> Setup {
    let url = url(port);
    match app {
        App::Claude => Setup {
            header: "# Claude Code: run once in a terminal".into(),
            text: format!(
                "claude mcp add --transport http {SERVER_NAME} {url} --header \"Authorization: Bearer {}\"",
                token.as_str()
            ),
            after: None,
        },
        App::Codex => Setup {
            header: "# ~/.codex/config.toml".into(),
            text: format!(
                "[mcp_servers.{SERVER_NAME}]\nurl = {}\nbearer_token_env_var = \"{TOKEN_VAR}\"",
                toml_str(&url)
            ),
            after: Some(export(token)),
        },
        App::Cursor => Setup {
            header: "// ~/.cursor/mcp.json".into(),
            text: cursor(Remote {
                url,
                headers: BTreeMap::from([("Authorization", format!("Bearer {}", token.as_str()))]),
            }),
            after: None,
        },
    }
}

pub fn hints(http: bool, config: Option<&Path>) -> String {
    let flag = if http { " --http" } else { "" };
    let c = config
        .map(|p| format!(" -c {}", path_arg(p)))
        .unwrap_or_default();
    let mut out = String::from("Ready-made setup for your AI app:\n");
    for (_, word, label) in APPS {
        out.push_str(&format!(
            "  {:<13} burrow mcp{c}{flag} --print-config {word}\n",
            format!("{label}:")
        ));
    }
    out
}

pub fn started(port: u16, token: Option<&Token>, config: Option<&Path>) -> String {
    let mut out = String::from("burrow mcp: MCP over HTTP\n");
    out.push_str(&format!("  URL:    {}\n", url(port)));
    let Some(token) = token else {
        return out;
    };
    out.push_str(&format!(
        "  Header: Authorization: Bearer {}\n\n",
        token.as_str()
    ));
    out.push_str(&hints(true, config));
    out
}

pub fn by_hand(config: Option<&Path>) -> String {
    format!(
        "burrow mcp speaks MCP on stdin and stdout and is meant to be started by your AI app; stop it with Ctrl-C\n\n{}",
        hints(false, config)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> Token {
        Token::from_stored("ab".repeat(32))
    }

    #[test]
    fn the_start_block_names_the_url_the_header_and_the_three_setups() {
        let text = started(7838, Some(&token()), None);
        assert_eq!(
            text,
            format!(
                "burrow mcp: MCP over HTTP\n  URL:    http://127.0.0.1:7838/mcp\n  Header: Authorization: Bearer {}\n\nReady-made setup for your AI app:\n  Claude Code:  burrow mcp --http --print-config claude\n  Codex:        burrow mcp --http --print-config codex\n  Cursor:       burrow mcp --http --print-config cursor\n",
                "ab".repeat(32)
            )
        );
        assert_eq!(
            started(7838, None, None),
            "burrow mcp: MCP over HTTP\n  URL:    http://127.0.0.1:7838/mcp\n"
        );
    }

    #[test]
    fn a_given_config_is_named_in_the_hints() {
        let h = hints(false, Some(Path::new("/srv/ops laptop.yaml")));
        assert!(
            h.contains("burrow mcp -c '/srv/ops laptop.yaml' --print-config claude")
                || cfg!(windows),
            "{h}"
        );
    }

    #[test]
    fn each_setup_is_valid_in_its_own_format() {
        let config = Path::new("/home/me/ops \"x\".yaml");
        for (app, _, _) in APPS {
            for s in [stdio(app, config), http(app, 7838, &token())] {
                match app {
                    App::Codex => {
                        let v: toml::Value = toml::from_str(&s.text).unwrap();
                        assert!(v["mcp_servers"]["burrow"].is_table(), "{}", s.text);
                    }
                    App::Cursor => {
                        let v: serde_json::Value = serde_json::from_str(&s.text).unwrap();
                        assert!(v["mcpServers"]["burrow"].is_object(), "{}", s.text);
                    }
                    App::Claude => {
                        let words = shlex::split(&s.text).unwrap();
                        assert_eq!(&words[..3], ["claude", "mcp", "add"], "{}", s.text);
                    }
                }
                assert!(s.header.starts_with("# ") || s.header.starts_with("// "));
            }
        }
        let codex: toml::Value = toml::from_str(&stdio(App::Codex, config).text).unwrap();
        assert_eq!(
            codex["mcp_servers"]["burrow"]["args"][2].as_str(),
            Some("/home/me/ops \"x\".yaml")
        );
        assert!(
            http(App::Codex, 7838, &token())
                .after
                .unwrap()
                .contains(TOKEN_VAR)
        );
    }
}
