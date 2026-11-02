use std::io::Write;
use std::time::Instant;

use super::record::{self, Record, Request};
use super::{allowed_target, outcome_of, window};
use crate::base::failure::Failure;
use crate::config::{Loaded, Role};
use crate::install::Manifest;
use crate::service::helper;
use crate::wire::protocol::Outcome;

fn opted_in(m: &Manifest) -> Result<(), String> {
    if !m.remote_updates {
        return Err("the service was installed without remote updates".into());
    }
    let env = crate::config::env::Env::with(crate::service::env_of(m));
    let loaded = crate::config::load_file(&m.config, Some(Role::Client), &env)
        .map_err(|e| e.describe(&m.config).0)?
        .loaded;
    match loaded {
        Some(Loaded::Client(s, _)) if s.remote_updates => Ok(()),
        Some(Loaded::Client(..)) => {
            Err(format!("remote updates are off in {}", m.config.display()))
        }
        _ => Err(format!(
            "{} is not a valid client config",
            m.config.display()
        )),
    }
}

fn shared_with_others(m: &Manifest) -> Result<(), String> {
    let others: Vec<String> = crate::install::all_manifests()
        .into_iter()
        .filter(|o| o.binary == m.binary && o.name != m.name && !helper::wanted(o))
        .map(|o| o.name)
        .collect();
    if others.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} also run {} and did not allow remote updates; update them all by hand with: {}",
        others.join(", "),
        m.binary.display(),
        crate::base::failure::admin("update")
    ))
}

pub type Probe<'a> = &'a dyn Fn(&std::path::Path, &str) -> bool;

fn healthy(m: &Manifest, version: &str, probe: Probe) -> Result<(), String> {
    let window = window();
    let until = Instant::now() + window;
    let config = crate::base::fs::canonical_or_same(&m.config);
    while Instant::now() < until {
        if probe(&config, version) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!(
        "{version} did not connect to the server within {} s",
        window.as_secs()
    ))
}

fn take_request(m: &Manifest) -> Option<Request> {
    let file = helper::request_file(m)?;
    let request = record::read::<Request>(&file);
    let _ = std::fs::remove_file(&file);
    request
}

fn finish(
    out: &mut dyn Write,
    m: &Manifest,
    r: Record,
    outcome: Outcome,
    why: String,
) -> Result<u8, Failure> {
    let result = helper::result_file(m);
    let _ = writeln!(
        out,
        "{}: {} -> {}: {}",
        m.name,
        r.from,
        r.to,
        outcome.wire()
    );
    record::write(&result, &r.ended(outcome, why), 0o644)
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", result.display())))?;
    Ok(u8::from(outcome != Outcome::Done))
}

pub fn run(name: &str, probe: Probe, out: &mut dyn Write) -> Result<u8, Failure> {
    let m = crate::service::resolve(Some(name))?;
    if m.role != Role::Client {
        return Err(Failure::usage(format!(
            "{} is not a client service",
            m.name
        )));
    }
    crate::update::use_env(crate::service::env_of(&m));
    let mut code = 0;
    while let Some(request) = take_request(&m) {
        code = one(&m, &request, probe, out)?;
    }
    Ok(code)
}

fn one(m: &Manifest, request: &Request, probe: Probe, out: &mut dyn Write) -> Result<u8, Failure> {
    let installed = crate::service::binary_version(&m.binary);
    let r = Record::new(request.id, &installed, &request.version, &m.config);
    let result = helper::result_file(m);
    let checks = opted_in(m)
        .and_then(|()| allowed_target(&request.version, &installed).map(drop))
        .and_then(|()| shared_with_others(m));
    if let Err(why) = checks {
        return finish(out, m, r, Outcome::Failed, why);
    }
    record::write(&result, &r, 0o644)
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", result.display())))?;
    let staged = match crate::update::fetch_verified(&request.version, &m.binary, out) {
        Ok(s) => s,
        Err(f) => return finish(out, m, r, Outcome::Failed, f.message),
    };
    let check = |s: &Manifest| healthy(s, &request.version, probe);
    match crate::service::update_checked(&m.name, staged.binary(), &request.version, &check, out) {
        Ok(()) => finish(out, m, r, Outcome::Done, String::new()),
        Err(f) => finish(out, m, r, outcome_of(&f.message), f.message),
    }
}
