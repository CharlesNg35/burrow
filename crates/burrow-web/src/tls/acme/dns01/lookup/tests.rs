use super::super::fake::{Fake, World, cname, soa};
use super::super::wire::{Reach, name, shown};
use super::*;

async fn follow(resolver: &Fake, start: &str) -> Result<String, String> {
    let resolvers = vec![resolver.addr];
    let reach = Reach::default();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &reach,
    };
    lookup
        .follow(&name(start).unwrap())
        .await
        .map(|n| shown(&n))
}

#[tokio::test]
async fn an_acme_challenge_cname_is_followed_to_its_final_target() {
    let r = Fake::start(false).await;
    assert_eq!(
        follow(&r, "_acme-challenge.wiki.example.com")
            .await
            .unwrap(),
        "_acme-challenge.wiki.example.com"
    );
    r.add(cname(
        "_acme-challenge.erp.corp.example.org",
        "erp.acme.example.com",
    ));
    r.add(cname("erp.acme.example.com", "final.acme.example.com"));
    assert_eq!(
        follow(&r, "_acme-challenge.erp.corp.example.org")
            .await
            .unwrap(),
        "final.acme.example.com"
    );
}

#[tokio::test]
async fn eight_cnames_are_followed_and_a_ninth_or_a_loop_is_an_error() {
    let r = Fake::start(false).await;
    for i in 0..8 {
        r.add(cname(
            &format!("c{i}.example.com"),
            &format!("c{}.example.com", i + 1),
        ));
    }
    assert_eq!(
        follow(&r, "c0.example.com").await.unwrap(),
        "c8.example.com"
    );
    r.add(cname("c8.example.com", "c9.example.com"));
    let err = follow(&r, "c0.example.com").await.unwrap_err();
    assert!(
        err.starts_with(
            "c0.example.com follows more than 8 CNAMEs (c0.example.com → c1.example.com"
        ),
        "{err}"
    );
    r.add(cname("loop-a.example.com", "loop-b.example.com"));
    r.add(cname("loop-b.example.com", "loop-a.example.com"));
    assert_eq!(
        follow(&r, "loop-a.example.com").await.unwrap_err(),
        "loop-a.example.com is a CNAME loop: loop-a.example.com → loop-b.example.com → loop-a.example.com"
    );
}

#[tokio::test]
async fn the_zone_and_its_nameservers_are_found_through_the_resolvers() {
    let w = World::start("example.com", 2).await;
    let resolvers = w.resolvers();
    let lookup = Lookup {
        resolvers: &resolvers,
        reach: &w.reach,
    };
    let zone = lookup
        .zone_of(&name("_acme-challenge.a.b.example.com").unwrap())
        .await
        .unwrap();
    assert_eq!(shown(&zone), "example.com");
    let servers = lookup.nameservers(&zone).await.unwrap();
    let shown_servers: Vec<(String, Vec<IpAddr>)> = servers
        .iter()
        .map(|s| (shown(&s.host), s.addrs.clone()))
        .collect();
    assert_eq!(
        shown_servers,
        vec![
            (
                "ns1.dns-host.test".into(),
                vec!["192.0.2.1".parse().unwrap()]
            ),
            (
                "ns2.dns-host.test".into(),
                vec!["192.0.2.2".parse().unwrap()]
            ),
        ]
    );
    let none = Lookup {
        resolvers: &[],
        reach: &w.reach,
    };
    let err = none.zone_of(&zone).await.unwrap_err();
    assert!(err.ends_with("none is set"), "{err}");
}

#[tokio::test]
async fn the_update_zone_is_the_first_soa_the_server_answers_with_authority() {
    let primary = Fake::start(true).await;
    primary.add(soa("apps.example.com"));
    let zone = authoritative_zone(
        primary.addr,
        &name("_acme-challenge.board.apps.example.com").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(shown(&zone), "apps.example.com");
    let other = Fake::start(false).await;
    other.add(soa("apps.example.com"));
    let none = authoritative_zone(
        other.addr,
        &name("_acme-challenge.x.apps.example.com").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(none, None);
}
