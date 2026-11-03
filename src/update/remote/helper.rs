use std::io::Write;

use super::record::{self, Record};
use super::{allowed_target, outcome_of, window};
use crate::base::failure::Failure;
use crate::config::{Loaded, Role};
use crate::install::Manifest;
use crate::service::helper;
use crate::wire::protocol::Outcome;

pub(super) fn opted_in(m: &Manifest) -> Result<(), String> {
    if !m.remote_updates {
        return Err(match m.role {
            Role::Server => format!(
                "the service was installed without remote updates; to allow them, set update.remote: true in server.yaml and run: {}",
                crate::base::failure::admin(&format!(
                    "service install -c {}",
                    crate::base::failure::path_arg(&m.source_config)
                ))
            ),
            _ => "the service was installed without remote updates".into(),
        });
    }
    match m.role {
        Role::Server => Ok(()),
        _ => client_config_allows(m),
    }
}

fn client_config_allows(m: &Manifest) -> Result<(), String> {
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

fn accepts(m: &Manifest, staged: &std::path::Path, to: &str) -> Result<(), String> {
    match m.role {
        Role::Server => Ok(()),
        _ => super::by_hand::accepts_config(staged, &m.config, to, &crate::service::env_of(m)),
    }
}

pub(super) fn shared_with_others(m: &Manifest, all: Vec<Manifest>) -> Result<(), String> {
    let others: Vec<String> = all
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

pub fn run(name: &str, out: &mut dyn Write) -> Result<u8, Failure> {
    let m = crate::service::resolve(Some(name))?;
    let Some(health) = super::health::of(m.role) else {
        return Err(Failure::usage(format!(
            "{} is a {} service, and only clients and servers update this way",
            m.name, m.role
        )));
    };
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
    if let Err(why) = shared_with_others(&m, crate::install::all_manifests()) {
        return finish(out, &m, r, Outcome::Failed, why);
    }
    record::write(&helper::result_file(&m), &r, 0o644)
        .map_err(|e| Failure::new(format!("cannot write the result: {e}")))?;
    let staged = match crate::update::fetch_verified(&to, &m.binary, out) {
        Ok(s) => s,
        Err(f) => return finish(out, &m, r, Outcome::Failed, f.message),
    };
    if let Err(why) = accepts(&m, staged.binary(), &to) {
        return finish(out, &m, r, Outcome::Failed, why);
    }
    health.before(&m);
    let check = |s: &Manifest| super::health::wait(&*health, s, &to, window());
    match crate::service::update_checked(&m.name, staged.binary(), &to, &check, out) {
        Ok(()) => {
            let note = health.note(&m, &to);
            finish(out, &m, r, Outcome::Done, note)
        }
        Err(f) => finish(out, &m, r, outcome_of(&f.message), f.message),
    }
}
