mod args;
mod ca;
mod service;

use std::path::PathBuf;

pub use args::{
    AdminArgs, ClientConfigArgs, EditArgs, ExportArgs, InitArgs, McpApp, McpArgs, RunArgs,
    ServerArgs, StatusArgs, UpdateArgs, ValidateArgs, VisitArgs, VisitorConfigArgs, parse_duration,
};
pub use ca::{CaAction, CaExportArgs};
use clap::{Parser, Subcommand, ValueEnum};
pub use service::{
    ServiceAction, ServiceControlArgs, ServiceInstallArgs, ServiceLogsArgs, ServiceName,
    ServiceUninstallArgs, ServiceUpdateArgs,
};

#[derive(Debug, Parser)]
#[command(name = "burrow", version, about = "Fast, secure reverse tunnel")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Info, help = "Log verbosity", help_heading = "Logging")]
    pub log_level: LogLevel,
    #[arg(
        long,
        global = true,
        help = "Write logs as JSON lines",
        help_heading = "Logging"
    )]
    pub log_json: bool,
    #[arg(
        long,
        short,
        global = true,
        help = "Only log warnings and errors",
        help_heading = "Logging"
    )]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help = "Write logs to this file (rotated at 10 MB, 5 old files kept)",
        help_heading = "Logging"
    )]
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    Server,
    Client,
    Visitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FileFormat {
    Yaml,
    Json,
}

pub const ADMIN_COMMAND: &str = "admin";
pub const MCP_COMMAND: &str = "mcp";

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Run the public server")]
    Server(ServerArgs),
    #[command(about = "Run the client behind NAT")]
    Client(RunArgs),
    #[command(about = "Forward local ports to private services through the server")]
    Visit(VisitArgs),
    #[command(about = "Check a config file and report every problem")]
    Validate(ValidateArgs),
    #[command(
        name = ADMIN_COMMAND,
        about = "Open the admin UI of a server in your browser, as a visitor with admin access"
    )]
    Admin(AdminArgs),
    #[command(
        name = MCP_COMMAND,
        about = "Let an AI app (Claude Code, Claude Desktop, Cursor) read and change a server over MCP, as a visitor with admin access"
    )]
    Mcp(McpArgs),
    #[command(
        about = "Show the running burrow servers on this machine: clients, services, visitors and traffic"
    )]
    Status(StatusArgs),
    #[command(
        about = "Open a config in your editor, check it, save it and apply it: one in this folder, of an installed service or of a running instance"
    )]
    Edit(EditArgs),
    #[command(about = "Write a new server, client or visitor config")]
    Init(InitArgs),
    #[command(about = "Print a ready client config for a client defined in the server config")]
    ClientConfig(ClientConfigArgs),
    #[command(about = "Print a ready visitor config for a visitor defined in the server config")]
    VisitorConfig(VisitorConfigArgs),
    #[command(about = "Print a new random 256-bit secret")]
    Gensecret,
    #[cfg(feature = "web")]
    #[command(
        about = "Ask for a password without showing it and print its hash for a website's basic_auth"
    )]
    Passwd {
        #[arg(
            value_name = "USER",
            help = "Print the line as USER: hash, ready for basic_auth"
        )]
        user: Option<String>,
    },
    #[command(about = "Print the JSON Schema of a config file")]
    Schema {
        #[arg(value_enum)]
        role: RoleArg,
    },
    #[command(about = "Print shell completions")]
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(about = "Print the version")]
    Version,
    #[command(
        about = "Download the latest release, replace this binary and update the installed services"
    )]
    Update(UpdateArgs),
    #[command(about = "Run burrow as a system service: install, status, logs, uninstall")]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    #[command(about = "Burrow's local certificate authority for tls: internal sites")]
    Ca {
        #[command(subcommand)]
        action: CaAction,
    },
}

pub fn service_word(
    e: &clap::Error,
    args: &[std::ffi::OsString],
) -> Option<crate::base::failure::Failure> {
    if e.kind() != clap::error::ErrorKind::InvalidSubcommand {
        return None;
    }
    let Some(clap::error::ContextValue::String(word)) =
        e.get(clap::error::ContextKind::InvalidSubcommand)
    else {
        return None;
    };
    if !["install", "uninstall", "start", "stop", "restart", "logs"].contains(&word.as_str()) {
        return None;
    }
    let rest: String = args
        .iter()
        .skip(1)
        .skip_while(|a| a.to_str() != Some(word.as_str()))
        .skip(1)
        .map(|a| format!(" {}", crate::base::failure::arg(&a.to_string_lossy())))
        .collect();
    let supported = crate::base::failure::SERVICES;
    Some(
        crate::base::failure::Failure::usage(format!("burrow {word} is not a command"))
            .hint(service_word_hint(word, &rest, supported)),
    )
}

fn service_word_hint(word: &str, rest: &str, supported: bool) -> String {
    if supported {
        format!(
            "for a service, run: {}",
            crate::base::failure::admin(&format!("service {word}{rest}"))
        )
    } else {
        "burrow cannot manage services on this operating system; run burrow server or burrow client under your own service manager".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_syntax::ports::{
        ADMIN_PORT, FIRST_SERVICE_PORT, MCP_PORT, SERVER_PORT, WS_PORT, WSS_PORT,
    };
    use clap::CommandFactory;

    fn help_of(cmd: &mut clap::Command, out: &mut String) {
        out.push_str(&cmd.render_long_help().to_string());
        for sub in cmd.get_subcommands_mut() {
            help_of(sub, out);
        }
    }

    fn numbers_after(text: &str, before: &str) -> Vec<u16> {
        text.match_indices(before)
            .filter_map(|(i, _)| {
                let rest = &text[i + before.len()..];
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .collect()
    }

    #[test]
    fn service_words_used_alone_point_to_the_service_command() {
        let parse = |args: &[&str]| {
            let e = Cli::try_parse_from(args).unwrap_err();
            let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
            service_word(&e, &args)
        };
        let supported = crate::base::failure::SERVICES;
        for word in ["install", "uninstall", "start", "stop", "restart", "logs"] {
            let f = parse(&["burrow", word]).unwrap_or_else(|| panic!("{word}"));
            assert_eq!(f.message, format!("burrow {word} is not a command"));
            let hint = f.hint.unwrap();
            if supported {
                assert!(hint.contains(&format!("burrow service {word}")), "{hint}");
            } else {
                assert!(hint.starts_with("burrow cannot manage services"), "{hint}");
            }
        }
        let hint = |args: &[&str]| parse(args).unwrap().hint.unwrap();
        if supported {
            assert!(hint(&["burrow", "stop", "home"]).contains("burrow service stop home"));
            assert!(
                hint(&["burrow", "--log-level", "debug", "logs", "home", "-n", "3"])
                    .contains("burrow service logs home -n 3")
            );
            assert!(
                hint(&["burrow", "install", "-c", "my home.yaml"]).contains(&format!(
                    "burrow service install -c {}",
                    crate::base::failure::arg("my home.yaml")
                ))
            );
        }
        assert!(
            service_word_hint("stop", " home", false).starts_with("burrow cannot manage services"),
        );
        assert!(service_word_hint("stop", " home", true).contains("service stop home"));
        for args in [&["burrow", "instal"][..], &["burrow", "service", "frob"]] {
            assert!(parse(args).is_none(), "{args:?}");
        }
    }

    #[test]
    fn help_and_schema_text_name_burrow_s_ports_by_their_constants() {
        let mut text = String::new();
        help_of(&mut Cli::command(), &mut text);
        for role in [
            crate::config::Role::Server,
            crate::config::Role::Client,
            crate::config::Role::Visitor,
        ] {
            text.push_str(&crate::config::load::schema_json(role).to_string());
        }
        let family = [
            SERVER_PORT,
            WS_PORT,
            WSS_PORT,
            ADMIN_PORT,
            MCP_PORT,
            FIRST_SERVICE_PORT,
        ];
        let bytes = text.as_bytes();
        for (i, _) in text.match_indices("78") {
            let boundary = |j: usize| j >= bytes.len() || !bytes[j].is_ascii_digit();
            if (i == 0 || boundary(i - 1)) && boundary(i + 4) && i + 4 <= bytes.len() {
                let n: u16 = text[i..i + 4].parse().unwrap_or(0);
                if (7800..7900).contains(&n) {
                    assert!(family.contains(&n), "{n} is not one of burrow's ports");
                }
            }
        }
        for (before, want) in [
            ("port ", SERVER_PORT),
            ("like 0.0.0.0:", SERVER_PORT),
            ("default 127.0.0.1:", WS_PORT),
            ("for example 127.0.0.1:", WS_PORT),
            ("or 0.0.0.0:", WSS_PORT),
            ("session's port or ", ADMIN_PORT),
            ("for --http; ", MCP_PORT),
        ] {
            let found = numbers_after(&text, before);
            assert!(!found.is_empty(), "no text names a port after {before:?}");
            for n in found.into_iter().filter(|n| (7800..7900).contains(n)) {
                assert_eq!(n, want, "after {before:?}");
            }
        }
    }
}
