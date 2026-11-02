use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use burrow_admin_proto::updates::{UpdateClients, UpdateClientsReply};

use super::call::{self, Fetch};
use super::query::{Found, Pick, Scan, did_not_answer, fetch, scan};
use super::render::{Paint, versions};
use super::watch::Watch;
use crate::base::failure::Failure;
use crate::base::prompt::Prompter;
use crate::config::Role;
use crate::instance::publish::UPDATE_PATH;

const POLL: Duration = Duration::from_secs(1);
const ASK_DEADLINE: Duration = Duration::from_secs(30);
const FROM_A_VISITOR: &str =
    "from a visitor, update clients in burrow admin or with burrow mcp's update_clients";

pub struct Update {
    pub names: Vec<String>,
    pub all: bool,
    pub yes: bool,
}

pub struct Options {
    pub pick: Pick,
    pub json: bool,
    pub update: Option<Update>,
}

fn refuse(what: &str, role: Role) -> Failure {
    let why = match role {
        Role::Visitor => {
            format!("{what} is a visitor, and burrow clients runs on the server machine")
        }
        _ => format!("{what} is a client, and burrow clients runs on the server machine"),
    };
    Failure::usage(why).hint(FROM_A_VISITOR)
}

fn wanted(pick: &Pick) -> Result<Option<PathBuf>, Failure> {
    let (path, role, what) = match pick {
        Pick::All => return Ok(None),
        Pick::Config(p) => (p.clone(), crate::edit::role_of(p), p.display().to_string()),
        Pick::Service(n) => {
            let m = crate::service::resolve(Some(n))?;
            (
                m.config.clone(),
                Some(m.role),
                format!("service {}", m.name),
            )
        }
    };
    match role {
        Some(r @ (Role::Visitor | Role::Client)) => Err(refuse(&what, r)),
        _ => Ok(Some(crate::base::fs::canonical_or_same(&path))),
    }
}

fn none(scan: &Scan, pick: &Pick) -> Failure {
    if let Some((entry, why)) = scan.silent.first() {
        return Failure::new(did_not_answer(entry, why))
            .hint("it may be busy; try again in a moment, or look at its logs");
    }
    let what = match pick {
        Pick::Config(p) => format!("no running burrow server uses {}", p.display()),
        Pick::Service(n) => format!("service {n} is not running"),
        Pick::All => "no burrow server is running on this machine".to_string(),
    };
    let f = Failure::new(what);
    if scan.hidden > 0 {
        return f.hint(format!(
            "{} running server(s) belong to another user, such as a service; run: {}",
            scan.hidden,
            crate::base::failure::admin("clients")
        ));
    }
    f.hint(FROM_A_VISITOR)
}

pub fn servers(dir: &Path, pick: &Pick) -> Result<Vec<Found>, Failure> {
    let wanted = wanted(pick)?;
    let mut scan = scan(dir, wanted.as_deref());
    scan.found.retain(|f| f.status.client.is_none());
    if scan.found.is_empty() {
        return Err(none(&scan, pick));
    }
    Ok(scan.found)
}

fn named(f: &Found) -> String {
    format!(
        "{} (pid {})",
        super::query::clean(&f.entry.config.display().to_string()),
        f.entry.pid
    )
}

pub fn one(found: Vec<Found>) -> Result<Found, Failure> {
    if found.len() == 1 {
        return Ok(found.into_iter().next().expect("one server"));
    }
    let list: Vec<String> = found.iter().map(|f| format!("  {}", named(f))).collect();
    Err(Failure::usage(format!(
        "{} burrow servers run on this machine; pick one with --service NAME or -c FILE:\n{}",
        found.len(),
        list.join("\n")
    )))
}

fn say(out: &mut dyn Write, text: &str) -> Result<(), Failure> {
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Failure::new(format!("cannot write output: {e}")))
}

fn list(found: &[Found], json: bool, paint: Paint, out: &mut dyn Write) -> Result<u8, Failure> {
    if json {
        let list: Vec<serde_json::Value> = found
            .iter()
            .map(|f| {
                serde_json::json!({
                    "config": f.entry.config,
                    "pid": f.entry.pid,
                    "latest": f.status.latest,
                    "clients": f.status.clients.iter().filter(|c| !c.local).collect::<Vec<_>>(),
                })
            })
            .collect();
        say(
            out,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&list).expect("list serializes")
            ),
        )?;
        return Ok(0);
    }
    let many = found.len() > 1;
    let mut text = String::new();
    for f in found {
        if many {
            text.push_str(&format!("\n== {}", named(f)));
        }
        text.push_str(&versions(&f.status, paint));
    }
    say(out, &format!("{}\n", text.trim_start_matches('\n')))?;
    Ok(0)
}

fn ask(server: &Found, r: &UpdateClients) -> Result<UpdateClientsReply, Failure> {
    let body = serde_json::to_vec(r).expect("request serializes");
    let answer = call::blocking(
        Instant::now() + ASK_DEADLINE,
        call::call(&server.entry, "POST", UPDATE_PATH, body),
    );
    let (code, body) = answer.map_err(|(kind, why)| match kind {
        Fetch::Gone => Failure::new("the server stopped"),
        Fetch::Failed => Failure::new(did_not_answer(&server.entry, &why)),
    })?;
    if code == 200 {
        return serde_json::from_slice(&body)
            .map_err(|e| Failure::new(format!("the server's answer is not an update reply: {e}")));
    }
    let why = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(super::query::clean))
        .unwrap_or_else(|| call::answered(code));
    Err(match code {
        404 | 405 => Failure::new("this server cannot update clients from its command line")
            .hint("update burrow on the server first: sudo burrow update"),
        _ => Failure::new(why),
    })
}

fn plan(reply: &UpdateClientsReply) -> String {
    let mut text = String::new();
    for s in &reply.skipped {
        text.push_str(&format!("{}: not asked: {}\n", s.client, s.why));
    }
    if !reply.asked.is_empty() {
        let how = if reply.asked.len() > 1 {
            ", one at a time, stopping at the first that does not come back"
        } else {
            ""
        };
        text.push_str(&format!(
            "update {} to {}{how}\n",
            reply.asked.join(", "),
            reply.to
        ));
    }
    text
}

fn update(
    server: Found,
    u: &Update,
    json: bool,
    out: &mut dyn Write,
    prompt: &mut dyn Prompter,
) -> Result<u8, Failure> {
    let wanted = UpdateClients {
        clients: u.names.clone(),
        all: u.all,
        version: None,
        dry_run: true,
    };
    let preview = ask(&server, &wanted)?;
    if !json {
        say(out, &plan(&preview))?;
    }
    if preview.asked.is_empty() {
        if json {
            say(
                out,
                &format!(
                    "{}\n",
                    serde_json::json!({"to": preview.to, "asked": [], "skipped": preview.skipped, "results": {}})
                ),
            )?;
        }
        return Ok(1);
    }
    if !u.yes {
        if !prompt.interactive() {
            return Err(Failure::usage(
                "nothing was asked: confirm in a terminal, or add --yes",
            ));
        }
        if !prompt.confirm("go ahead?", false) {
            return Ok(1);
        }
    }
    let before =
        fetch(&server.entry).map_err(|why| Failure::new(did_not_answer(&server.entry, &why)))?;
    let reply = ask(
        &server,
        &UpdateClients {
            clients: preview.asked.clone(),
            all: false,
            version: Some(preview.to.clone()),
            dry_run: false,
        },
    )?;
    if reply.asked.is_empty() {
        if !json {
            say(out, &plan(&reply))?;
        }
        return Ok(1);
    }
    if !json {
        say(
            out,
            "Ctrl-C stops watching; the update goes on on the server\n",
        )?;
    }
    let mut watch = Watch::new(&reply.to, &reply.asked, &before);
    loop {
        let st = fetch(&server.entry)
            .map_err(|_| Failure::new("the server stopped while clients were updating"))?;
        let (lines, done) = watch.step(&st);
        if !json {
            for l in lines {
                say(out, &format!("{}\n", super::query::clean(&l)))?;
            }
        }
        if done {
            break;
        }
        std::thread::sleep(POLL);
    }
    if json {
        let results: serde_json::Map<String, serde_json::Value> = watch
            .results
            .iter()
            .map(|(n, o)| {
                (
                    n.clone(),
                    serde_json::json!({"state": o.state, "detail": o.detail}),
                )
            })
            .collect();
        say(out, &format!("{}\n", serde_json::to_string_pretty(&serde_json::json!({"to": reply.to, "asked": reply.asked, "skipped": preview.skipped, "results": results})).expect("result serializes")))?;
    }
    Ok(if watch.ok() { 0 } else { 1 })
}

pub fn run_in(
    dir: &Path,
    opts: Options,
    out: &mut dyn Write,
    prompt: &mut dyn Prompter,
) -> Result<u8, Failure> {
    let found = servers(dir, &opts.pick)?;
    let paint = super::query::screen(opts.json);
    match opts.update {
        None => list(&found, opts.json, paint, out),
        Some(u) => update(one(found)?, &u, opts.json, out, prompt),
    }
}

pub fn run(opts: Options, out: &mut dyn Write, prompt: &mut dyn Prompter) -> Result<u8, Failure> {
    run_in(&crate::instance::default_dir(), opts, out, prompt)
}

#[cfg(test)]
mod tests;
