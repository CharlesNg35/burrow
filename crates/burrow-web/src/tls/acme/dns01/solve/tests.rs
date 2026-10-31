use std::sync::{Arc, Mutex};

use super::super::fake::{Fake, World, cname};
use super::super::{DNS_WAIT, Fut, Secret};
use super::*;

struct Writer {
    ns: Arc<Fake>,
    zone: String,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Provider for Writer {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("present {} {value}", fqdn.to_ascii()));
            if !shown(fqdn).ends_with(&self.zone) {
                return Err(Refusal::NotWritable("not my zone".into()));
            }
            self.ns
                .add(super::super::fake::txt(&fqdn.to_ascii(), value));
            Ok(())
        })
    }

    fn cleanup<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("cleanup {} {value}", fqdn.to_ascii()));
            self.ns.remove(&fqdn.to_ascii(), value);
            Ok(())
        })
    }
}

fn entry(domains: &[&str]) -> DnsEntry {
    DnsEntry {
        domains: domains.iter().map(|d| d.to_string()).collect(),
        provider: DnsProvider::Cloudflare {
            token: Secret::new("t".into()),
        },
        wait: Duration::from_secs(3),
        resolvers: Vec::new(),
        wildcards: Vec::new(),
    }
}

struct Setup {
    world: World,
    dns: Dns,
    calls: Arc<Mutex<Vec<String>>>,
    auth: Arc<Fake>,
    _state: tempfile::TempDir,
}

async fn setup(domains: &[&str], writable: &str) -> Setup {
    let mut world = World::start("acme.example.com", 1).await;
    let auth = Arc::new(world.ns.remove(0));
    let state = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut dns = Dns::new(
        &[entry(domains)],
        world.reach.clone(),
        Some(state.path().to_path_buf()),
    )
    .with_system(world.resolvers());
    dns.every = Duration::from_millis(100);
    dns.providers[0] = Ok(Box::new(Writer {
        ns: auth.clone(),
        zone: writable.into(),
        calls: calls.clone(),
    }));
    Setup {
        world,
        dns,
        calls,
        auth,
        _state: state,
    }
}

#[tokio::test]
async fn a_cname_target_the_entry_can_write_gets_the_record_and_it_is_removed_after() {
    let s = setup(&["corp.example.org"], "acme.example.com").await;
    s.world.resolver.add(cname(
        "_acme-challenge.erp.corp.example.org",
        "erp.acme.example.com",
    ));
    let mut set = Vec::new();
    let items = vec![
        ("erp.corp.example.org".to_string(), "v1".to_string()),
        ("*.erp.corp.example.org".to_string(), "v2".to_string()),
    ];
    s.dns.present(&items, &mut set).await.unwrap();
    assert_eq!(s.auth.values("erp.acme.example.com"), vec!["v1", "v2"]);
    let left = pending::load(&pending::path(s._state.path()));
    assert_eq!(left.len(), 2);
    s.dns.wait(&set).await.unwrap();
    s.dns.cleanup(&set).await;
    assert!(s.auth.values("erp.acme.example.com").is_empty());
    assert!(!pending::path(s._state.path()).exists());
}

#[tokio::test]
async fn a_cname_target_the_entry_cannot_write_names_the_fix() {
    let s = setup(&["corp.example.org"], "nothing.test").await;
    s.world.resolver.add(cname(
        "_acme-challenge.erp.corp.example.org",
        "erp.acme.example.com",
    ));
    let mut set = Vec::new();
    let err = s
        .dns
        .present(&[("erp.corp.example.org".into(), "v".into())], &mut set)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "_acme-challenge.erp.corp.example.org is a CNAME to erp.acme.example.com, which dns[0] (Cloudflare) cannot update: list corp.example.org on the entry that manages acme.example.com"
    );
    assert!(set.is_empty());
    assert!(!pending::path(s._state.path()).exists());
    s.world
        .resolver
        .add(cname("*.lab.corp.example.org", "x.provider.net"));
    let err = s
        .dns
        .present(&[("lab.corp.example.org".into(), "v".into())], &mut set)
        .await
        .unwrap_err();
    assert!(
        err.ends_with("_acme-challenge.lab.corp.example.org is answered by a wildcard CNAME (*.lab.corp.example.org); add an explicit _acme-challenge record or remove the wildcard"),
        "{err}"
    );
    let err = s
        .dns
        .present(&[("wiki.example.net".into(), "v".into())], &mut set)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "wiki.example.net needs a DNS check, but no web.letsencrypt.dns entry lists its domain"
    );
}

#[tokio::test]
async fn records_left_by_a_crash_are_removed_on_the_next_start() {
    let s = setup(&["acme.example.com"], "acme.example.com").await;
    let path = pending::path(s._state.path());
    let left = Left {
        name: "wiki.acme.example.com".into(),
        fqdn: "_acme-challenge.wiki.acme.example.com.".into(),
        value: "old".into(),
        since: now_secs(),
    };
    pending::add(&path, &left).unwrap();
    s.auth.add(super::super::fake::txt(&left.fqdn, "old"));
    let gone = Left {
        name: "x.example.net".into(),
        ..left.clone()
    };
    pending::add(
        &path,
        &Left {
            value: "gone".into(),
            ..gone
        },
    )
    .unwrap();
    s.dns.replay().await;
    assert!(s.auth.values(&left.fqdn).is_empty());
    assert_eq!(
        *s.calls.lock().unwrap(),
        vec!["cleanup _acme-challenge.wiki.acme.example.com. old".to_string()]
    );
    assert!(!path.exists());
}

#[test]
fn the_challenge_name_drops_the_wildcard_and_takes_the_default_wait() {
    assert_eq!(
        challenge_name("*.Example.com").unwrap().to_ascii(),
        "_acme-challenge.Example.com."
    );
    assert_eq!(DNS_WAIT, Duration::from_secs(120));
}
