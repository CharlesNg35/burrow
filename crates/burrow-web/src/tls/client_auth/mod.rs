use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use tokio_rustls::rustls::RootCertStore;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::server::danger::ClientCertVerifier;
use tokio_rustls::rustls::server::{
    ResolvesServerCert, VerifierBuilderError, WebPkiClientVerifier,
};
use tracing::warn;

pub use self::crl::{ClientAuthProblem, CrlNote};
use crate::ClientAuth;

mod crl;

pub type Verifier = Arc<dyn ClientCertVerifier>;

struct Loaded {
    verifier: Verifier,
    expiry: Vec<(String, SystemTime)>,
}

fn load(ca: &[u8], crl: Option<&[u8]>) -> Result<Loaded, ClientAuthProblem> {
    let cas = crl::read_cas(ca)?;
    let crls = crl.map(crl::read_crls).transpose()?.unwrap_or_default();
    let mut expiry = Vec::new();
    for c in &crls {
        expiry.push(crl::check_crl(c.as_ref(), &cas)?);
    }
    let mut roots = RootCertStore::empty();
    for c in &cas {
        roots
            .add(c.der.clone())
            .map_err(|_| ClientAuthProblem::NotCa)?;
    }
    let mut b =
        WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::new(default_provider()));
    if !crls.is_empty() {
        b = b.with_crls(crls).allow_unknown_revocation_status();
    }
    let verifier = b.build().map_err(|e| match e {
        VerifierBuilderError::InvalidCrl(_) => ClientAuthProblem::NotCrl,
        _ => ClientAuthProblem::NotCa,
    })?;
    Ok(Loaded { verifier, expiry })
}

fn notes_at(expiry: &[(String, SystemTime)], now: SystemTime) -> Vec<CrlNote> {
    expiry
        .iter()
        .filter(|(_, next)| *next <= now)
        .map(|(issuer, next)| CrlNote::Expired {
            issuer: issuer.clone(),
            next_update: *next,
        })
        .collect()
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn check_files(
    ca: &Path,
    crl: Option<&Path>,
    now: SystemTime,
) -> Result<Vec<CrlNote>, ClientAuthProblem> {
    let ca = read(ca).map_err(ClientAuthProblem::CaUnreadable)?;
    let crl = crl
        .map(read)
        .transpose()
        .map_err(ClientAuthProblem::CrlUnreadable)?;
    let loaded = load(&ca, crl.as_deref())?;
    Ok(notes_at(&loaded.expiry, now))
}

struct Held {
    files: (Vec<u8>, Option<Vec<u8>>),
    verifier: Option<Verifier>,
    configs: Option<[Arc<ServerConfig>; 2]>,
    expiry: Vec<(String, SystemTime)>,
    kept: Option<String>,
}

pub(crate) struct Verifiers {
    resolver: Arc<dyn ResolvesServerCert>,
    held: RwLock<HashMap<ClientAuth, Held>>,
}

fn files(key: &ClientAuth) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let ca = read(&key.ca).map_err(|e| format!("client_auth.ca: cannot read {e}"))?;
    let crl = key
        .crl
        .as_deref()
        .map(read)
        .transpose()
        .map_err(|e| format!("client_auth.crl: cannot read {e}"))?;
    Ok((ca, crl))
}

fn described(key: &ClientAuth, p: &ClientAuthProblem) -> String {
    match (p, &key.crl) {
        (ClientAuthProblem::CaUnreadable(_) | ClientAuthProblem::NotCa, _) | (_, None) => {
            format!("client_auth.ca {} {p}", key.ca.display())
        }
        (_, Some(crl)) => format!("client_auth.crl {} {p}", crl.display()),
    }
}

impl Verifiers {
    pub(crate) fn new(resolver: Arc<dyn ResolvesServerCert>) -> Verifiers {
        Verifiers {
            resolver,
            held: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn refresh<'a>(&self, wanted: impl IntoIterator<Item = &'a ClientAuth>) {
        let mut held = self.held.write().expect("verifiers lock");
        let mut next = HashMap::new();
        for key in wanted {
            if next.contains_key(key) {
                continue;
            }
            let old = held.remove(key);
            next.insert(key.clone(), self.renew(key, old));
        }
        *held = next;
    }

    fn renew(&self, key: &ClientAuth, old: Option<Held>) -> Held {
        let read = files(key);
        if let (Ok(f), Some(o)) = (&read, &old)
            && *f == o.files
        {
            return old.expect("checked above");
        }
        let built = read.and_then(|f| {
            load(&f.0, f.1.as_deref())
                .map(|l| (f, l))
                .map_err(|p| described(key, &p))
        });
        match (built, old) {
            (Ok((files, l)), _) => Held {
                configs: Some([false, true].map(|h2| {
                    crate::terminate::site_config(
                        self.resolver.clone(),
                        Some(l.verifier.clone()),
                        h2,
                    )
                })),
                files,
                verifier: Some(l.verifier),
                expiry: l.expiry,
                kept: None,
            },
            (Err(why), Some(mut o)) => {
                warn!("web: {why}; the last good files stay in force");
                o.kept = Some(why);
                o
            }
            (Err(why), None) => {
                warn!("web: {why}; visitors of these names are refused until it is fixed");
                Held {
                    files: (Vec::new(), None),
                    verifier: None,
                    configs: None,
                    expiry: Vec::new(),
                    kept: Some(why),
                }
            }
        }
    }

    pub(crate) fn config(&self, key: &ClientAuth, http2: bool) -> Option<Arc<ServerConfig>> {
        let held = self.held.read().expect("verifiers lock");
        let [h1, h2] = held.get(key)?.configs.as_ref()?;
        Some(if http2 { h2.clone() } else { h1.clone() })
    }

    pub(crate) fn verifier(&self, key: &ClientAuth) -> Option<Verifier> {
        self.held
            .read()
            .expect("verifiers lock")
            .get(key)?
            .verifier
            .clone()
    }

    pub(crate) fn notes(&self, now: SystemTime) -> Vec<(ClientAuth, String)> {
        let held = self.held.read().expect("verifiers lock");
        let mut out: Vec<(ClientAuth, String)> = Vec::new();
        for (key, h) in held.iter() {
            out.extend(
                notes_at(&h.expiry, now)
                    .into_iter()
                    .map(|n| (key.clone(), n.to_string())),
            );
            if let Some(why) = &h.kept {
                let text = if h.verifier.is_some() {
                    format!("{why}; the last good files stay in force")
                } else {
                    format!("{why}; visitors are refused until it is fixed")
                };
                out.push((key.clone(), text));
            }
        }
        out.sort_by(|a, b| (&a.0.ca, &a.1).cmp(&(&b.0.ca, &b.1)));
        out
    }
}

#[cfg(test)]
mod tests;
