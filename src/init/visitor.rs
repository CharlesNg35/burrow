use std::io::Write;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::export::{ConfigOutput, VISITOR, connect_address, load_server, pick_name, secret_text};
use super::{InitOptions, Login, ask_login, resolve_output, write_new};
use crate::base::failure::Failure;
use crate::base::prompt::Prompter;
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::{Format, Role};

pub fn init_visitor(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    if opts.update {
        return update_visitor(opts, prompt, out);
    }
    let (path, format) = resolve_output(Role::Visitor, opts)?;
    let Login {
        server,
        name,
        secret_text,
    } = ask_login(Role::Visitor, &path, opts, prompt)?;
    let bare = document(
        format,
        &server,
        &name,
        &secret_text,
        &Map::new(),
        &BTreeMap::new(),
    );
    let later = |why: &str| {
        let update = match &opts.output {
            Some(p) => format!(
                "init visitor --update -o {}",
                crate::base::failure::path_arg(p)
            ),
            None => "init visitor --update".to_string(),
        };
        format!(
            "wrote {} without forwards: {why}\nadd them later with: {}\nor ask the server's admin for a ready config: {}",
            path.display(),
            crate::base::failure::cmd(&update),
            crate::base::failure::cmd(&format!("visitor-config {name} -o visitor.yaml"))
        )
    };
    let settings = match settings_of(&path, &bare, format) {
        Ok(s) => s,
        Err(f) => {
            write_new(&path, &bare, opts.force)?;
            let why = format!("{}, so the server was not asked", f.message);
            return say(out, &later(&why)).map(|()| path);
        }
    };
    let fetched = match super::fetch::grants(&settings) {
        Ok(g) => g,
        Err(n) if !n.rejected => {
            write_new(&path, &bare, opts.force)?;
            return say(out, &later(&n.failure.message)).map(|()| path);
        }
        Err(n) => return Err(n.failure),
    };
    let grants = fetched.grants;
    let planned = super::plan::plan(&grants, &BTreeMap::new(), true, fetched.tcp_only);
    let notes = allow_notes(&grants, &planned);
    write_new(
        &path,
        &document(
            format,
            &server,
            &name,
            &secret_text,
            &planned.forwards,
            &notes,
        ),
        opts.force,
    )?;
    if planned.forwards.is_empty() && planned.unplaced.is_empty() {
        return say(
            out,
            &format!(
                "wrote {}; the server lets visitor \"{name}\" use nothing yet",
                path.display()
            ),
        )
        .map(|()| path);
    }
    let mut text = format!(
        "wrote {} with {} {}:\n",
        path.display(),
        planned.forwards.len(),
        crate::diagnostics::plural(planned.forwards.len(), "forward")
    );
    text.push_str(&listing(&planned.forwards, &notes, |_| ("", "")));
    if let Some(line) = left_out(&planned) {
        text.push_str(&format!("{line}\n"));
    }
    if fetched.tcp_only {
        text.push_str(&format!("  {}\n", super::fetch::OLD_SERVER_NOTE));
    }
    text.push_str(&format!(
        "run them with: {}",
        crate::base::failure::cmd("visit")
    ));
    say(out, &text).map(|()| path)
}

fn update_visitor(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let given = match &opts.output {
        Some(p) => p.clone(),
        None => crate::config::discover::discover_with(Path::new("."), Some(Role::Visitor), "-o")?,
    };
    let path = crate::base::fs::canonical_or_same(&given);
    let format = Format::from_path(&path).ok_or_else(|| {
        Failure::usage(format!("cannot tell the format of {}", path.display()))
            .hint("visitor configs end in .yaml, .yml or .json")
    })?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Failure::new(format!(
            "cannot read {}: {}",
            path.display(),
            crate::base::failure::file_reason(&e)
        ))
    })?;
    let settings = settings_of(&path, &text, format)?;
    let fetched = super::fetch::grants(&settings).map_err(Failure::from)?;
    let grants = fetched.grants;
    let planned = super::plan::plan(&grants, &settings.forwards, true, fetched.tcp_only);
    let old_server = if fetched.tcp_only {
        format!("\n{}", super::fetch::OLD_SERVER_NOTE)
    } else {
        String::new()
    };
    let notes = allow_notes(&grants, &planned);
    let stale: Map<String, Value> = match format {
        Format::Yaml => planned
            .forwards
            .iter()
            .filter(|(k, _)| !planned.added.contains(k))
            .filter(|(k, _)| {
                notes.get(k.as_str()).is_some_and(|n| {
                    let line = format!("# {n}");
                    !text.lines().any(|l| l.trim() == line)
                })
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        Format::Json => Map::new(),
    };
    if !planned.changed() && stale.is_empty() {
        let mut text = format!(
            "{} is up to date ({} {})",
            path.display(),
            planned.forwards.len(),
            crate::diagnostics::plural(planned.forwards.len(), "forward")
        );
        if let Some(line) = left_out(&planned) {
            text.push_str(&format!("\n{line}"));
        }
        text.push_str(&old_server);
        return say(out, &text).map(|()| path);
    }
    let mut changes = String::new();
    for name in planned
        .removed
        .iter()
        .filter(|n| !planned.added.contains(n))
    {
        changes.push_str(&format!("  - {name}  (no longer granted)\n"));
    }
    let added: Map<String, Value> = planned
        .forwards
        .iter()
        .filter(|(k, _)| planned.added.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    changes.push_str(&listing(&added, &notes, |k| {
        if planned.removed.contains(&k.to_string()) {
            ("~ ", "  (changed)")
        } else {
            ("+ ", "  (new)")
        }
    }));
    changes.push_str(&listing(&stale, &notes, |_| {
        ("~ ", "  (what it can reach changed)")
    }));
    if let Some(line) = left_out(&planned) {
        changes.push_str(&format!("{line}\n"));
    }
    changes.push_str(&old_server);
    say(out, changes.trim_end_matches('\n'))?;
    if !opts.yes {
        if !prompt.interactive() {
            say(
                out,
                "not written; run again with --yes to write these changes",
            )?;
            return Ok(path);
        }
        if !prompt.confirm(&format!("update {}?", path.display()), true) {
            say(out, "not written")?;
            return Ok(path);
        }
    }
    let edit = super::splice::Edit {
        removed: &planned.removed,
        added: &added,
        notes: &notes,
    };
    let next = match format {
        Format::Yaml => super::splice::update_yaml(&text, &edit),
        Format::Json => super::splice::update_json(&text, &edit),
    }
    .ok_or_else(|| {
        Failure::new(format!(
            "burrow cannot rewrite the forwards of {}",
            path.display()
        ))
        .hint("edit them by hand, or write a new file with: burrow init visitor -o FILE")
    })?;
    if settings_of(&path, &next, format).is_err() {
        return Err(Failure::new(format!(
            "the updated {} would not load, so it was not written",
            path.display()
        ))
        .hint("this is likely a burrow bug; please report it with the file (without its secret)"));
    }
    crate::edit::save::write_back(&path, &text, &next).map_err(|e| match e {
        crate::edit::save::Refused::Changed => Failure::new(format!(
            "{} changed while burrow was asking the server; nothing was written",
            path.display()
        ))
        .hint("run the update again"),
        crate::edit::save::Refused::Io(e) => Failure::new(format!(
            "cannot write {}: {}",
            path.display(),
            crate::base::failure::file_reason(&e)
        )),
    })?;
    say(
        out,
        &format!(
            "updated {}; a running burrow visit applies it by itself",
            path.display()
        ),
    )
    .map(|()| path)
}

fn say(out: &mut dyn Write, text: &str) -> Result<(), Failure> {
    writeln!(out, "{text}").map_err(|e| Failure::new(format!("cannot write output: {e}")))
}

fn settings_of(
    path: &Path,
    text: &str,
    format: Format,
) -> Result<crate::config::settings::VisitorSettings, Failure> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let outcome = crate::config::load::load_str_in(
        dir,
        &path.display().to_string(),
        text,
        format,
        Some(Role::Visitor),
        &crate::config::env::ProcessEnv,
    );
    let first = outcome
        .report
        .diagnostics
        .iter()
        .find(|d| d.is_error())
        .map(|d| d.message.clone());
    match outcome.loaded {
        Some(crate::config::Loaded::Visitor(s, _)) if first.is_none() => Ok(*s),
        _ => Err(Failure::new(format!(
            "{} does not load as a visitor config: {}",
            path.display(),
            first.unwrap_or_else(|| "it is not a visitor config".into())
        ))
        .hint(format!(
            "see why with: {}",
            crate::base::failure::cmd(&format!(
                "validate -c {}",
                crate::base::failure::path_arg(path)
            ))
        ))),
    }
}

fn allow_notes(
    grants: &crate::wire::protocol::Grants,
    planned: &super::plan::Plan,
) -> BTreeMap<String, String> {
    planned
        .allows
        .keys()
        .filter_map(|key| {
            let client = planned.forwards.get(key)?.get("client")?.as_str()?;
            let granted = grants.clients.iter().find(|c| c.name == client)?;
            Some((key.clone(), super::splice::reach_note(granted)))
        })
        .collect()
}

fn document(
    format: Format,
    server: &str,
    name: &str,
    secret: &str,
    forwards: &Map<String, Value>,
    notes: &BTreeMap<String, String>,
) -> String {
    match format {
        Format::Json => super::splice::new_json(server, name, secret, forwards),
        Format::Yaml => {
            let head = render(
                &json!({ "server": server, "name": name, "secret": secret }),
                Format::Yaml,
            );
            format!("{head}{}", super::splice::forwards_yaml(forwards, notes))
        }
    }
}

fn listing(
    forwards: &Map<String, Value>,
    notes: &BTreeMap<String, String>,
    tag: impl Fn(&str) -> (&'static str, &'static str),
) -> String {
    let width = forwards
        .keys()
        .map(|k| k.chars().count())
        .max()
        .unwrap_or(0);
    forwards
        .iter()
        .map(|(key, v)| {
            let local = match &v["local"] {
                Value::Number(n) => format!("127.0.0.1:{n}"),
                Value::String(s) if s.contains(':') => s.clone(),
                Value::String(s) => format!("127.0.0.1:{s}"),
                _ => String::new(),
            };
            let local = match v.get("protocol").and_then(Value::as_str) {
                Some("udp") => format!("udp {local}"),
                _ => local,
            };
            let to = match (v.get("service"), v.get("client"), v.get("target")) {
                (Some(s), _, _) => format!("service {}", s.as_str().unwrap_or("")),
                (_, Some(c), Some(t)) => format!(
                    "{} on client {}",
                    t.as_str().unwrap_or(""),
                    c.as_str().unwrap_or("")
                ),
                (_, Some(c), None) => {
                    format!("SOCKS5 proxy into client {}", c.as_str().unwrap_or(""))
                }
                _ => String::new(),
            };
            let note = match (notes.get(key), v.get("client").and_then(Value::as_str)) {
                (Some(n), Some(c)) => {
                    format!(" ({})", n.trim_start_matches(&format!("client {c} ")))
                }
                _ => String::new(),
            };
            let (mark, label) = tag(key);
            format!("  {mark}{key:<width$}  {local} -> {to}{note}{label}\n")
        })
        .collect()
}

fn left_out(planned: &super::plan::Plan) -> Option<String> {
    let names = &planned.unplaced;
    (!names.is_empty()).then(|| {
        format!(
            "  no free local ports for {}; add {} by hand",
            names.join(", "),
            if names.len() == 1 { "it" } else { "them" }
        )
    })
}

pub fn socks_note(client: &str, local: u16, reach: &[String]) -> String {
    let target = reach
        .iter()
        .find_map(|r| match r.parse() {
            Ok(crate::allowlist::Rule::Net(rule)) => Some(rule.example().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "HOST:PORT".to_string());
    format!(
        "127.0.0.1:{local} is a SOCKS5 proxy into client \"{client}\": point an app at it, or forward one target with: {} visit --client {client} --target {target} --local 2222",
        crate::base::failure::command()
    )
}

pub fn visitor_config(
    name: Option<&str>,
    server_config: &Path,
    server: Option<&str>,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ConfigOutput, Failure> {
    let settings = load_server(server_config, env)?;
    let names: Vec<&str> = settings.visitors.keys().map(String::as_str).collect();
    let name = pick_name(VISITOR, name, &names, server_config, server)?;
    let admin = settings.visitors[&name].admin.is_some();
    if !reaches_something(&settings, &name) && !admin {
        return Err(Failure::new(format!(
            "visitor \"{name}\" cannot reach anything in {}",
            server_config.display()
        ))
        .hint(format!(
            "add it to a service, for example visitors: [{name}], or give it reach: {{ CLIENT: any }}"
        )));
    }
    let auth = &settings.visitors[&name];
    let mut notes = Vec::new();
    let server = connect_address(VISITOR, &settings, server, server_config, &mut notes)?;
    let secret_text = secret_text(VISITOR, server_config, &name, &auth.secrets);
    let value = visitor_document(&settings, &name, &server, &secret_text, &mut notes);
    if admin {
        notes.push(format!(
            "visitor \"{name}\" has admin access: open the admin UI with {}",
            crate::base::failure::cmd("admin -c FILE")
        ));
    }
    Ok(ConfigOutput {
        text: render(&value, format),
        notes,
    })
}

pub fn reaches_something(settings: &crate::config::settings::ServerSettings, name: &str) -> bool {
    crate::config::grants::for_visitor(settings, name)
        .is_some_and(|g| !g.services.is_empty() || !g.clients.is_empty())
}

pub fn visitor_document(
    settings: &crate::config::settings::ServerSettings,
    name: &str,
    server: &str,
    secret_text: &str,
    notes: &mut Vec<String>,
) -> Value {
    let Some(grants) = crate::config::grants::for_visitor(settings, name) else {
        return json!({ "server": server, "name": name, "secret": secret_text });
    };
    let planned = super::plan::plan(&grants, &std::collections::BTreeMap::new(), false, false);
    for (_, client, local) in &planned.socks {
        let reach = grants
            .clients
            .iter()
            .find(|c| &c.name == client)
            .map_or(&[][..], |c| &c.reach[..]);
        notes.push(socks_note(client, *local, reach));
    }
    if let Some(line) = left_out(&planned) {
        notes.push(line.trim_start().to_string());
    }
    let forwards = planned.forwards;
    json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "forwards": Value::Object(forwards),
    })
}
