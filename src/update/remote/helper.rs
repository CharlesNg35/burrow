use std::io::Write;
use std::time::Instant;

use super::record::{self, Record};
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

pub type Probe<'a> = &'a dyn Fn(&Manifest, &str) -> bool;

fn healthy(m: &Manifest, version: &str, probe: Probe) -> Result<(), String> {
    let window = window();
    let until = Instant::now() + window;
    while Instant::now() < until {
        if probe(m, version) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!(
        "{version} did not connect to the server within {} s",
        window.as_secs()
    ))
}

fn latest() -> Result<String, String> {
    crate::update::latest::check()
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
    helper::take_trigger(&m);
    crate::update::use_env(crate::service::env_of(&m));
    let installed = crate::service::binary_version(&m.binary);
    let mut r = Record::new(0, &installed, "", &m.config);
    if let Err(why) = opted_in(&m) {
        return finish(out, &m, r, Outcome::Failed, why);
    }
    let to = match latest().and_then(|v| allowed_target(&v, &installed).map(|v| v.to_string())) {
        Ok(v) => v,
        Err(why) => return finish(out, &m, r, Outcome::Failed, why),
    };
    r.to = to.clone();
    if let Err(why) = shared_with_others(&m) {
        return finish(out, &m, r, Outcome::Failed, why);
    }
    record::write(&helper::result_file(&m), &r, 0o644)
        .map_err(|e| Failure::new(format!("cannot write the result: {e}")))?;
    let staged = match crate::update::fetch_verified(&to, &m.binary, out) {
        Ok(s) => s,
        Err(f) => return finish(out, &m, r, Outcome::Failed, f.message),
    };
    let env = crate::service::env_of(&m);
    if let Err(why) = super::by_hand::accepts_config(staged.binary(), &m.config, &to, &env) {
        return finish(out, &m, r, Outcome::Failed, why);
    }
    let check = |s: &Manifest| healthy(s, &to, probe);
    match crate::service::update_checked(&m.name, staged.binary(), &to, &check, out) {
        Ok(()) => finish(out, &m, r, Outcome::Done, String::new()),
        Err(f) => finish(out, &m, r, outcome_of(&f.message), f.message),
    }
}
