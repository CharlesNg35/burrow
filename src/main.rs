#![forbid(unsafe_code)]

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use burrow::cli::{Cli, Command, FileFormat, OutputFormat, RoleArg};
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

fn run(cli: Cli) -> Result<u8, Failure> {
    let out_style = Style::detect(std::io::stdout().is_terminal());
    let err_style = Style::detect(std::io::stderr().is_terminal());
    match cli.command {
        Command::Validate(args) => {
            let v = burrow::validate::validate(
                &args.config,
                args.role.map(role),
                args.strict,
                &ProcessEnv,
            )?;
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
            };
            let mut out = std::io::stdout().lock();
            match args.role {
                RoleArg::Server => burrow::init::init_server(&opts, &mut out)?,
                RoleArg::Client => burrow::init::init_client(&opts, &mut TtyPrompt, &mut out)?,
            };
            Ok(0)
        }
        Command::ClientConfig(args) => {
            let r = burrow::init::client_config(
                &args.name,
                &args.config,
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
        Command::Server(_) | Command::Client(_) => Err(Failure::new("not implemented yet")),
    }
}
