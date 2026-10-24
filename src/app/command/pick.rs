use crate::base::failure::Failure;

pub(super) fn visit_pick(
    args: crate::app::cli::VisitArgs,
) -> Result<crate::visitor::Pick, Failure> {
    use crate::visitor::Pick;
    use crate::wire::protocol::VisitTarget;
    use burrow_config::{Bind, Target};
    let local = match &args.local {
        Some(l) => Some(
            crate::config::check::parse_local(&Bind::Addr(l.clone())).map_err(|e| {
                Failure::usage(format!("invalid --local: {e}"))
                    .hint("use a port like 5432 (on 127.0.0.1), a range like 40000-40010, or ip:port like 127.0.0.1:5432")
            })?,
        ),
        None => None,
    };
    if let Some((l, n)) = local.filter(|(l, _)| l.ip().is_unspecified()) {
        let ports = crate::config::check::ports::show(l.port(), n);
        crate::say_errln!(
            "warning: --local {} listens on all interfaces: other machines on your network get this visitor's access (use --local {ports} for this machine only)",
            crate::config::check::ports::show_addr(l, n)
        );
    }
    let name_ok = |flag: &str, name: &str| {
        if crate::config::check::valid_name(name) {
            Ok(())
        } else {
            Err(Failure::usage(format!("invalid {flag} \"{name}\""))
                .hint("use 1-64 letters, digits, \"-\", \"_\" or \".\""))
        }
    };
    if let Some(socks) = &args.socks {
        let client = args.client.clone().unwrap_or_default();
        name_ok("--client", &client)?;
        let local = match crate::config::check::parse_local(&Bind::Addr(socks.clone())) {
            Ok((addr, 1)) => addr,
            _ => {
                return Err(Failure::usage(format!("invalid --socks \"{socks}\"")).hint(
                    "use one port like 1080 (on 127.0.0.1), or ip:port like 127.0.0.1:1080",
                ));
            }
        };
        if local.ip().is_unspecified() {
            crate::say_errln!(
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
            crate::base::failure::cmd("visit --client home --target 22 --local 2222"),
            crate::base::failure::cmd("visit --client home --socks 1080")
        )));
    }
    if args.udp && args.service.is_none() && args.target.is_none() {
        return Err(Failure::usage(
            "--udp needs --service NAME or --client NAME --target HOST:PORT, with --local PORT",
        )
        .hint(format!(
            "for example: {}",
            crate::base::failure::cmd("visit --udp --service dns --local 5353")
        )));
    }
    if args.udp
        && args
            .target
            .as_deref()
            .is_some_and(|t| crate::config::check::unix::path_of(t).is_some())
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
            let checked = match crate::config::check::unix::path_of(&target) {
                Some(path) => {
                    crate::config::check::unix::check_path(path).map(|()| (target.clone(), 1))
                }
                None => crate::config::check::ports::split_target(&target).and_then(|(base, n)| {
                    crate::config::check::parse_host_port(&base).map(|_| (base, n))
                }),
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
                    crate::config::diagnostics::plural(ports.into(), "port")
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
                        crate::base::failure::cmd("visit db --local 5433")
                    ),
                ),
            );
        }
    })
}
