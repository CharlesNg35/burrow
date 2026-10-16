#![forbid(unsafe_code)]

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use burrow::cli::{Cli, Command, FileFormat, LogLevel, OutputFormat, RoleArg, ServiceAction};
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Role};
use burrow::diagnostics::Style;
use burrow::failure::Failure;
use burrow::init::{InitOptions, TtyPrompt};

fn role(r: RoleArg) -> Role {
    match r {
        RoleArg::Server => Role::Server,
        RoleArg::Client => Role::Client,
    }
}

fn format(f: FileFormat) -> Format {
    match f {
        FileFormat::Yaml => Format::Yaml,
        FileFormat::Json => Format::Json,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let err_style = Style::detect(std::io::stderr().is_terminal());
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(f) => {
            eprintln!("{}", f.render(err_style));
            ExitCode::from(f.exit)
        }
    }
}

fn print(s: &str) -> Result<(), Failure> {
    let mut out = std::io::stdout().lock();
    match out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::new(format!("cannot write output: {e}"))),
    }
}

fn config_path(
    given: Option<std::path::PathBuf>,
    role: Option<Role>,
) -> Result<std::path::PathBuf, Failure> {
    match given {
        Some(p) => Ok(p),
        None => burrow::config::discover::discover(std::path::Path::new("."), role),
    }
}

fn service(action: ServiceAction, logging: burrow::run::Logging) -> Result<u8, Failure> {
    use burrow::service;
    let mut out = std::io::stdout().lock();
    match action {
        ServiceAction::Install(a) => {
            let config = config_path(a.config, None)?;
            service::install(
                &service::InstallOptions {
                    config,
                    name: a.name,
                    run_as: a.run_as,
                    start: !a.no_start,
                    print: a.print,
                },
                &mut out,
            )?;
            Ok(0)
        }
        ServiceAction::Update(a) => {
            service::update(a.name.as_deref(), a.force, &mut out).map(|_| 0)
        }
        ServiceAction::Uninstall(a) => {
            service::uninstall(a.name.as_deref(), a.purge, &mut out).map(|_| 0)
        }
        ServiceAction::Start(n) => {
            service::control("start", n.name.as_deref(), &mut out).map(|_| 0)
        }
        ServiceAction::Stop(n) => service::control("stop", n.name.as_deref(), &mut out).map(|_| 0),
        ServiceAction::Restart(n) => {
            service::control("restart", n.name.as_deref(), &mut out).map(|_| 0)
        }
        ServiceAction::Reload(n) => {
            service::control("reload", n.name.as_deref(), &mut out).map(|_| 0)
        }
        ServiceAction::Status(n) => service::status(n.name.as_deref(), &mut out),
        ServiceAction::Logs(a) => service::logs(a.name.as_deref(), a.lines, a.follow).map(|_| 0),
        ServiceAction::List => service::list(&mut out).map(|_| 0),
        ServiceAction::Run(n) => {
            let _ = logging;
            service::run_service(n.name.as_deref().unwrap_or("burrow"))
        }
    }
}

fn run(cli: Cli) -> Result<u8, Failure> {
    let out_style = Style::detect(std::io::stdout().is_terminal());
    let err_style = Style::detect(std::io::stderr().is_terminal());
    let level = match (cli.quiet, cli.log_level) {
        (true, _) | (_, LogLevel::Warn) => tracing::Level::WARN,
        (_, LogLevel::Error) => tracing::Level::ERROR,
        (_, LogLevel::Info) => tracing::Level::INFO,
        (_, LogLevel::Debug) => tracing::Level::DEBUG,
        (_, LogLevel::Trace) => tracing::Level::TRACE,
    };
    let logging = burrow::run::Logging {
        level,
        json: cli.log_json,
        file: cli.log_file.clone(),
    };
    match cli.command {
        Command::Validate(args) => {
            let path = config_path(args.config, args.role.map(role))?;
            let v =
                burrow::validate::validate(&path, args.role.map(role), args.strict, &ProcessEnv)?;
            let text = match args.format {
                OutputFormat::Text => {
                    burrow::validate::render_text(&v.outcome, args.strict, out_style)
                }
                OutputFormat::Json => burrow::validate::render_json(&v.outcome, args.strict),
            };
            print(&text)?;
            Ok(v.exit)
        }
        Command::Schema { role: r } => {
            let schema = burrow::config::load::schema_json(role(r));
            print(&format!(
                "{}\n",
                serde_json::to_string_pretty(&schema).expect("schema serializes")
            ))?;
            Ok(0)
        }
        Command::Gensecret => {
            print(&format!(
                "{}\n",
                burrow::secret::Secret::generate().encode()
            ))?;
            Ok(0)
        }
        Command::Version => {
            print(&format!("burrow {}\n", env!("CARGO_PKG_VERSION")))?;
            Ok(0)
        }
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "burrow", &mut buf);
            print(&String::from_utf8_lossy(&buf))?;
            Ok(0)
        }
        Command::Init(args) => {
            let opts = InitOptions {
                format: Some(format(args.format)),
                output: args.output,
                force: args.force,
                server: args.server,
                name: args.name,
                client_name: args.client_name,
                secret: args.secret,
                env_secret: args.env_secret,
                websocket: (args.ws
                    || args.ws_listen.is_some()
                    || args.ws_path.is_some()
                    || args.ws_tls_cert.is_some())
                .then_some(burrow::init::WebSocketInit {
                    listen: args.ws_listen,
                    path: args.ws_path,
                    tls_cert: args.ws_tls_cert,
                    tls_key: args.ws_tls_key,
                }),
            };
            let mut out = std::io::stdout().lock();
            match args.role {
                RoleArg::Server => burrow::init::init_server(&opts, &mut out)?,
                RoleArg::Client => burrow::init::init_client(&opts, &mut TtyPrompt, &mut out)?,
            };
            Ok(0)
        }
        Command::ClientConfig(args) => {
            let path = config_path(args.config, Some(Role::Server))?;
            let r = burrow::init::client_config(
                &args.name,
                &path,
                &args.server,
                format(args.format),
                &ProcessEnv,
            )?;
            print(&r.text)?;
            for n in r.notes {
                eprintln!(
                    "{} {n}",
                    if err_style.color {
                        "\x1b[1;33mnote:\x1b[0m"
                    } else {
                        "note:"
                    }
                );
            }
            Ok(0)
        }
        Command::Service { action } => service(action, logging),
        Command::Server(args) => {
            burrow::run::server(&config_path(args.config, Some(Role::Server))?, logging)
        }
        Command::Client(args) => {
            burrow::run::client(&config_path(args.config, Some(Role::Client))?, logging)
        }
    }
}
