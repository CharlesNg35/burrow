#![forbid(unsafe_code)]

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use burrow::base::failure::Failure;
use burrow::base::prompt::TtyPrompt;
use burrow::base::style::Style;
use burrow::cli::{Cli, Command, FileFormat, LogLevel, OutputFormat, RoleArg, ServiceAction};
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Role};
use burrow::init::InitOptions;

fn role(r: RoleArg) -> Role {
    match r {
        RoleArg::Server => Role::Server,
        RoleArg::Client => Role::Client,
        RoleArg::Visitor => Role::Visitor,
    }
}

fn format(f: FileFormat) -> Format {
    match f {
        FileFormat::Yaml => Format::Yaml,
        FileFormat::Json => Format::Json,
    }
}

fn main() -> ExitCode {
    let err_style = Style::detect(std::io::stderr().is_terminal());
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => match burrow::cli::service_word(&e, &std::env::args_os().collect::<Vec<_>>()) {
            Some(f) => {
                burrow::say_errln!("{}", f.render(err_style));
                return ExitCode::from(f.exit);
            }
            None => e.exit(),
        },
    };
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(f) => {
            burrow::say_errln!("{}", f.render(err_style));
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

fn ask_role() -> Result<RoleArg, Failure> {
    use burrow::base::prompt::Prompter;
    let mut prompt = TtyPrompt;
    for _ in 0..3 {
        if !prompt.interactive() {
            break;
        }
        let answer = prompt
            .ask("which config? server (the public machine), client (the machine behind NAT) or visitor (a machine that uses private services)");
        match answer.map(|a| a.to_ascii_lowercase()).as_deref() {
            Some("server" | "s") => return Ok(RoleArg::Server),
            Some("client" | "c") => return Ok(RoleArg::Client),
            Some("visitor" | "v") => return Ok(RoleArg::Visitor),
            _ => {}
        }
    }
    Err(
        Failure::usage("burrow init needs to know which config to write").hint(format!(
            "run: {0} init server  (on the public machine), {0} init client  (behind NAT), or: {0} init visitor  (to use private services)",
            burrow::base::failure::command()
        )),
    )
}

type Export = fn(
    Option<&str>,
    &std::path::Path,
    Option<&str>,
    Format,
    &dyn burrow::config::env::EnvSource,
) -> Result<burrow::init::ConfigOutput, Failure>;

fn export(
    make: Export,
    name: Option<&str>,
    args: burrow::cli::ExportArgs,
    role: &str,
    err_style: Style,
) -> Result<u8, Failure> {
    let path = config_path(args.config, Some(Role::Server))?;
    let file_format = burrow::init::output_format(args.format.map(format), args.output.as_deref())?;
    let r = make(
        name,
        &path,
        args.server.as_deref(),
        file_format,
        &ProcessEnv,
    )?;
    match &args.output {
        Some(out) => {
            burrow::init::write_new(out, &r.text, args.force)?;
            burrow::say_errln!("wrote {}; copy it to the {role} machine", out.display());
        }
        None => print(&r.text)?,
    }
    for n in r.notes {
        burrow::say_errln!(
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

fn visit_pick(args: burrow::cli::VisitArgs) -> Result<burrow::visitor::Pick, Failure> {
    use burrow::visitor::Pick;
    use burrow::wire::protocol::VisitTarget;
    use burrow_config::{Bind, Target};
    let local = match &args.local {
        Some(l) => Some(
            burrow::config::check::parse_local(&Bind::Addr(l.clone())).map_err(|e| {
                Failure::usage(format!("invalid --local: {e}"))
                    .hint("use a port like 5432 (on 127.0.0.1), a range like 40000-40010, or ip:port like 127.0.0.1:5432")
            })?,
        ),
        None => None,
    };
    if let Some((l, n)) = local.filter(|(l, _)| l.ip().is_unspecified()) {
        let ports = burrow::config::check::ports::show(l.port(), n);
        burrow::say_errln!(
            "warning: --local {} listens on all interfaces: other machines on your network get this visitor's access (use --local {ports} for this machine only)",
            burrow::config::check::ports::show_addr(l, n)
        );
    }
    let name_ok = |flag: &str, name: &str| {
        if burrow::config::check::valid_name(name) {
            Ok(())
        } else {
            Err(Failure::usage(format!("invalid {flag} \"{name}\""))
                .hint("use 1-64 letters, digits, \"-\", \"_\" or \".\""))
        }
    };
    if let Some(socks) = &args.socks {
        let client = args.client.clone().unwrap_or_default();
        name_ok("--client", &client)?;
        let local = match burrow::config::check::parse_local(&Bind::Addr(socks.clone())) {
            Ok((addr, 1)) => addr,
            _ => {
                return Err(Failure::usage(format!("invalid --socks \"{socks}\"")).hint(
                    "use one port like 1080 (on 127.0.0.1), or ip:port like 127.0.0.1:1080",
                ));
            }
        };
        if local.ip().is_unspecified() {
            burrow::say_errln!(
                "warning: --socks {local} listens on all interfaces: other machines on your network get this visitor's access (use --socks {} for this machine only)",
                local.port()
            );
        }
        return Ok(Pick::Socks { client, local });
    }
    if args.client.is_some() && args.target.is_none() {
        return Err(Failure::usage(
            "--client needs --target HOST:PORT and --local PORT, or --socks PORT",
        )
        .hint(format!(
            "for example: {}, or: {}",
            burrow::base::failure::cmd("visit --client home --target 22 --local 2222"),
            burrow::base::failure::cmd("visit --client home --socks 1080")
        )));
    }
    if args.udp && args.service.is_none() && args.target.is_none() {
        return Err(Failure::usage(
            "--udp needs --service NAME or --client NAME --target HOST:PORT, with --local PORT",
        )
        .hint(format!(
            "for example: {}",
            burrow::base::failure::cmd("visit --udp --service dns --local 5353")
        )));
    }
    if args.udp
        && args
            .target
            .as_deref()
            .is_some_and(|t| burrow::config::check::unix::path_of(t).is_some())
    {
        return Err(
            Failure::usage("--udp cannot reach a Unix socket, which carries tcp only")
                .hint("give --target as host:port, or leave out --udp"),
        );
    }
    let adhoc = match (args.service, args.client, args.target) {
        (Some(service), _, _) => {
            name_ok("--service", &service)?;
            Some(VisitTarget::service(service))
        }
        (None, Some(client), Some(target)) => {
            name_ok("--client", &client)?;
            let target = Target::Addr(target).host_port();
            let checked = match burrow::config::check::unix::path_of(&target) {
                Some(path) => {
                    burrow::config::check::unix::check_path(path).map(|()| (target.clone(), 1))
                }
                None => {
                    burrow::config::check::ports::split_target(&target).and_then(|(base, n)| {
                        burrow::config::check::parse_host_port(&base).map(|_| (base, n))
                    })
                }
            };
            let (target, ports) = checked
                .map_err(|e| {
                    Failure::usage(format!("invalid --target: {e}"))
                        .hint("use host:port as seen from the client, for example 127.0.0.1:22, a port alone like 22, a range like 30000-30010, or unix:/path/to.sock")
                })?;
            if let Some((_, n)) = local
                && n != ports
            {
                return Err(Failure::usage(format!(
                    "--target has {ports} {} but --local has {n}",
                    burrow::diagnostics::plural(ports.into(), "port")
                ))
                .hint("give --local and --target ranges of the same size"));
            }
            Some(VisitTarget::Client { client, target })
        }
        _ => None,
    };
    Ok(match (adhoc, args.forward, local) {
        (Some(to), _, Some(local)) => Pick::Adhoc {
            to,
            local,
            protocol: if args.udp {
                burrow_config::Protocol::Udp
            } else {
                burrow_config::Protocol::Tcp
            },
        },
        (Some(_), _, None) => {
            return Err(Failure::usage("--service and --client need --local PORT"));
        }
        (None, Some(name), local) => Pick::Forward { name, local },
        (None, None, None) => Pick::All,
        (None, None, Some(_)) => {
            return Err(
                Failure::usage("--local needs a forward name, --service or --client").hint(
                    format!(
                        "for example: {}",
                        burrow::base::failure::cmd("visit db --local 5433")
                    ),
                ),
            );
        }
    })
}

fn control(
    action: &str,
    a: burrow::cli::ServiceControlArgs,
    out: &mut dyn std::io::Write,
) -> Result<u8, Failure> {
    use burrow::service;
    match &a.worker {
        Some(log) => Ok(service::as_worker(log, |o| {
            service::control_now(action, a.name.as_deref(), o)
        })),
        None => service::control(action, a.name.as_deref(), out).map(|_| 0),
    }
}

fn service(action: ServiceAction, logging: burrow::run::Logging) -> Result<u8, Failure> {
    use burrow::service;
    let mut out = std::io::stdout().lock();
    match action {
        ServiceAction::Install(a) => {
            let config = config_path(a.config, None)?;
            let opts = service::InstallOptions {
                config,
                name: a.name,
                run_as: a.run_as,
                start: !a.no_start,
                print: a.print,
                worker: a.worker,
                admin_writable: a.admin_writable,
            };
            match &opts.worker {
                Some(log) => Ok(service::as_worker(log, |out| service::install(&opts, out))),
                None => service::install(&opts, &mut out).map(|_| 0),
            }
        }
        ServiceAction::Update(a) => match (&a.worker, &a.from, &a.as_version) {
            (Some(log), Some(from), Some(version)) => Ok(service::update_worker(
                log,
                from,
                version,
                a.name.as_deref(),
                a.force,
            )),
            _ => service::update(a.name.as_deref(), a.force, &mut out).map(|_| 0),
        },
        ServiceAction::Uninstall(a) => {
            service::uninstall(a.name.as_deref(), a.purge, &mut out).map(|_| 0)
        }
        ServiceAction::Start(n) => {
            service::control("start", n.name.as_deref(), &mut out).map(|_| 0)
        }
        ServiceAction::Stop(n) => service::control("stop", n.name.as_deref(), &mut out).map(|_| 0),
        ServiceAction::Restart(a) => control("restart", a, &mut out),
        ServiceAction::Reload(a) => control("reload", a, &mut out),
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
            let v = match burrow::validate::validate(
                &path,
                args.role.map(role),
                args.strict,
                &ProcessEnv,
            ) {
                Ok(v) => v,
                Err(f) if matches!(args.format, OutputFormat::Json) => {
                    print(&burrow::validate::unreadable_json(&path, &f, args.strict))?;
                    return Ok(f.exit);
                }
                Err(f) => return Err(f),
            };
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
                burrow::wire::secret::Secret::generate().encode()
            ))?;
            Ok(0)
        }
        Command::Version => {
            print(&format!("burrow {}\n", env!("CARGO_PKG_VERSION")))?;
            Ok(0)
        }
        Command::Edit(a) => burrow::edit::run(
            &burrow::edit::Options {
                target: a.target,
                editor: a.editor,
            },
            &mut TtyPrompt,
            &mut std::io::stdout().lock(),
        ),
        Command::Status(a) => {
            use burrow::status::query::{Options, Pick, run};
            let pick = match (a.name, a.config) {
                (Some(name), _) => Pick::Service(name),
                (None, Some(config)) => Pick::Config(config),
                (None, None) => Pick::All,
            };
            if !(a.interval.is_finite() && (0.2..=3600.0).contains(&a.interval)) {
                return Err(Failure::usage(
                    "--interval must be between 0.2 and 3600 seconds",
                ));
            }
            let follow = a
                .follow
                .then(|| std::time::Duration::from_secs_f64(a.interval));
            let mut out = std::io::stdout().lock();
            run(
                Options {
                    pick,
                    json: a.json,
                    follow,
                },
                &mut out,
            )
        }
        Command::Update(a) => {
            let mut out = std::io::stdout().lock();
            burrow::update::run(
                &burrow::update::Options {
                    check: a.check,
                    to: a.to,
                    force: a.force,
                },
                &mut out,
            )
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
                format: args.format.map(format),
                output: args.output,
                force: args.force,
                server: args.server,
                name: args.name,
                client_name: args.client_name,
                secret: args.secret,
                env_secret: args.env_secret,
                update: args.update,
                yes: args.yes,
                websocket: (args.ws
                    || args.ws_listen.is_some()
                    || args.ws_path.is_some()
                    || args.ws_tls_cert.is_some()
                    || args.ws_tls_key.is_some())
                .then_some(burrow::init::WebSocketInit {
                    listen: args.ws_listen,
                    path: args.ws_path,
                    tls_cert: args.ws_tls_cert,
                    tls_key: args.ws_tls_key,
                }),
            };
            let role = match (args.role, args.update) {
                (Some(RoleArg::Visitor) | None, true) => RoleArg::Visitor,
                (Some(_), true) => {
                    return Err(
                        Failure::usage("--update is for visitor configs").hint(format!(
                            "run: {}",
                            burrow::base::failure::cmd("init visitor --update")
                        )),
                    );
                }
                (Some(r), false) => r,
                (None, false) => ask_role()?,
            };
            let given = [
                ("--name", opts.name.is_some()),
                ("--secret", opts.secret.is_some()),
                ("--client-name", opts.client_name.is_some()),
                ("--ws", opts.websocket.is_some()),
            ];
            let (kind, applies): (&str, &[&str]) = match role {
                RoleArg::Server => ("server", &["--client-name", "--ws"]),
                RoleArg::Client => ("client", &["--name", "--secret"]),
                RoleArg::Visitor => ("visitor", &["--name", "--secret"]),
            };
            if let Some((flag, _)) = given.iter().find(|(f, set)| *set && !applies.contains(f)) {
                let what = if *flag == "--ws" {
                    "--ws and its options are".to_string()
                } else {
                    format!("{flag} is")
                };
                return Err(Failure::usage(format!(
                    "{what} not for {kind} configs"
                ))
                .hint(match *flag {
                    "--name" | "--secret" => "for a server config, name the first client with --client-name; its secret is generated".to_string(),
                    "--client-name" => format!("the {kind}'s own name is --name"),
                    _ => "a client or visitor gets a WebSocket server from its --server URL, for example --server wss://tunnel.example.com/burrow".to_string(),
                }));
            }
            let mut out = std::io::stdout().lock();
            match role {
                RoleArg::Server => burrow::init::init_server(&opts, &mut out)?,
                RoleArg::Client => burrow::init::init_client(&opts, &mut TtyPrompt, &mut out)?,
                RoleArg::Visitor => burrow::init::init_visitor(&opts, &mut TtyPrompt, &mut out)?,
            };
            Ok(0)
        }
        Command::ClientConfig(args) => export(
            burrow::init::client_config,
            args.name.as_deref(),
            args.export,
            "client",
            err_style,
        ),
        Command::VisitorConfig(args) => export(
            burrow::init::visitor_config,
            args.name.as_deref(),
            args.export,
            "visitor",
            err_style,
        ),
        Command::Service { action } => service(action, logging),
        Command::Admin(args) => admin(args),
        Command::Server(args) => {
            let env = run_env(args.env_file.as_deref())?;
            burrow::run::server(&config_path(args.config, Some(Role::Server))?, logging, env)
        }
        Command::Client(args) => {
            let env = run_env(args.env_file.as_deref())?;
            burrow::run::client(&config_path(args.config, Some(Role::Client))?, logging, env)
        }
        Command::Visit(args) => {
            let path = config_path(args.config.clone(), Some(Role::Visitor))?;
            let env = run_env(args.env_file.as_deref())?;
            burrow::run::visit(&path, logging, visit_pick(args)?, env)
        }
    }
}

fn admin(args: burrow::cli::AdminArgs) -> Result<u8, Failure> {
    use burrow::admin_local::find::{Found, Installed, choose, unreadable};
    if args.ui_info {
        print!("{}", burrow::admin_local::assets::info());
        return Ok(0);
    }
    let here = burrow::config::discover::in_folder(std::path::Path::new("."))
        .into_iter()
        .filter(|(_, r)| *r == Role::Visitor)
        .map(|(p, _)| p)
        .next();
    let services: Vec<Installed> =
        if args.config.is_none() && (here.is_none() || args.service.is_some()) {
            burrow::install::all_manifests()
                .into_iter()
                .filter(|m| m.role == "visitor")
                .map(|m| Installed {
                    name: m.name,
                    config: m.config,
                    env_file: m.env_file,
                })
                .collect()
        } else {
            Vec::new()
        };
    let found = choose(args.config, here, &services, args.service.as_deref())?;
    let (path, env) = match found {
        Found::File(p) => (p, burrow::run::Env::default()),
        Found::Service(s) => {
            let readable = |p: &std::path::Path| {
                std::fs::File::open(p)
                    .map(drop)
                    .map_err(|e| unreadable(p, &e))
            };
            readable(&s.config)?;
            let env = match &s.env_file {
                Some(f) => {
                    readable(f)?;
                    burrow::service::saved_env(&s.name)
                }
                None => Default::default(),
            };
            println!(
                "using the installed visitor service {} ({})",
                s.name,
                s.config.display()
            );
            (s.config, burrow::run::Env::with(env))
        }
    };
    let opts = burrow::admin_local::Options {
        port: args.port,
        open: !args.no_open,
        print_url: args.print_url,
        url_file: args.url_file,
        idle_exit: args.idle_exit,
        ui_dir: args.ui_dir,
    };
    burrow::run::admin(&path, env, opts)
}

fn run_env(file: Option<&std::path::Path>) -> Result<burrow::run::Env, Failure> {
    match file {
        Some(f) => burrow::run::env_file(f),
        None => Ok(burrow::run::Env::default()),
    }
}
