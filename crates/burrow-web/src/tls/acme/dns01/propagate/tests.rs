use std::time::{Duration, Instant};

use hickory_proto::rr::RecordType;

use super::super::fake::{Mode, World, txt};
use super::super::lookup::Lookup;
use super::super::wire::name;
use super::wait_all;

const FQDN: &str = "_acme-challenge.wiki.example.com";
const EVERY: Duration = Duration::from_millis(100);

fn records(values: &[&str]) -> Vec<(hickory_proto::rr::Name, String)> {
    values
        .iter()
        .map(|v| (name(FQDN).unwrap(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn a_lagging_second_nameserver_delays_ready_until_it_answers_too() {
    let w = World::start("example.com", 2).await;
    w.ns[0].add(txt(FQDN, "v1"));
    w.ns[1].add_after(txt(FQDN, "v1"), Duration::from_millis(700));
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let start = Instant::now();
    wait_all(&lookup, &records(&["v1"]), Duration::from_secs(5), EVERY)
        .await
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(700));
    let asked_txt = |f: &super::super::fake::Fake| {
        f.asked()
            .iter()
            .filter(|(_, k)| *k == RecordType::TXT)
            .count()
    };
    assert!(asked_txt(&w.ns[1]) >= 2);
    assert_eq!(
        asked_txt(&w.resolver),
        0,
        "TXT is never asked of a resolver"
    );
}

#[tokio::test]
async fn a_wildcard_and_its_bare_name_wait_for_both_values() {
    let w = World::start("example.com", 1).await;
    w.ns[0].add(txt(FQDN, "bare"));
    w.ns[0].add_after(txt(FQDN, "wild"), Duration::from_millis(400));
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let start = Instant::now();
    wait_all(
        &lookup,
        &records(&["bare", "wild"]),
        Duration::from_secs(5),
        EVERY,
    )
    .await
    .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(400));
}

#[tokio::test]
async fn a_nameserver_that_never_answers_times_out_by_name() {
    let w = World::start("example.com", 2).await;
    w.ns[0].add(txt(FQDN, "v1"));
    w.ns[1].mode(Mode::Silent);
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let start = Instant::now();
    let err = wait_all(&lookup, &records(&["v1"]), Duration::from_secs(1), EVERY)
        .await
        .unwrap_err();
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert_eq!(
        err,
        "the TXT record _acme-challenge.wiki.example.com did not appear on ns2.dns-host.test (did not answer) within 1 s"
    );
}

#[tokio::test]
async fn a_value_that_never_arrives_names_the_nameserver_still_without_it() {
    let w = World::start("example.com", 1).await;
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let err = wait_all(
        &lookup,
        &records(&["v1"]),
        Duration::from_millis(300),
        EVERY,
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("on ns1.dns-host.test (not yet) within 0 s"),
        "{err}"
    );
}

#[tokio::test]
async fn a_refusing_or_non_authoritative_nameserver_is_an_error_at_once() {
    let w = World::start("example.com", 1).await;
    w.ns[0].mode(Mode::Refuse);
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let start = Instant::now();
    let err = wait_all(&lookup, &records(&["v1"]), Duration::from_secs(5), EVERY)
        .await
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(
        err,
        "the nameserver ns1.dns-host.test (192.0.2.1) refused the question for _acme-challenge.wiki.example.com"
    );
    let lame = World::start("example.com", 1).await;
    let auth = super::super::fake::Fake::start(false).await;
    auth.add(txt(FQDN, "v1"));
    let mut reach = lame.reach.clone();
    reach
        .redirect
        .insert("192.0.2.1".parse().unwrap(), auth.addr);
    let resolvers = lame.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &reach,
    };
    let err = wait_all(&lookup, &records(&["v1"]), Duration::from_secs(5), EVERY)
        .await
        .unwrap_err();
    assert!(err.contains("without authority"), "{err}");
}
