use super::*;
use burrow_admin_proto::status::{
    CertSourceStatus, CertStatus, ClientStatus, HealthStatus, ServiceStatus,
};

fn client(name: &str, connected: bool, left: Option<u64>) -> ClientStatus {
    ClientStatus {
        name: name.into(),
        connected,
        left_secs_ago: left,
        from: connected.then(|| "203.0.113.5:40000".to_string()),
        ..ClientStatus::default()
    }
}

fn status(clients: Vec<ClientStatus>) -> Status {
    Status {
        clients,
        ..Status::default()
    }
}

fn key(kind: Kind, name: &str) -> (Kind, String) {
    (kind, name.to_string())
}

#[test]
fn a_client_that_never_came_since_the_start_is_not_reported() {
    let seen = seen(&status(vec![client("home", false, None)]));
    assert_eq!(seen[&key(Kind::Client, "home")], Health::Unknown);
    let mut w = Watch::default();
    let t0 = Instant::now();
    assert!(w.step(t0, seen.clone()).is_empty());
    assert!(w.step(t0 + Duration::from_secs(3600), seen).is_empty());
}

#[test]
fn an_offline_client_is_told_once_after_a_minute_and_once_when_back() {
    let mut w = Watch::default();
    let t0 = Instant::now();
    let gone = seen(&status(vec![client("home", false, Some(1))]));
    assert!(
        w.step(t0, gone.clone()).is_empty(),
        "a minute's grace first"
    );
    assert!(
        w.step(t0 + Duration::from_secs(30), gone.clone())
            .is_empty()
    );
    let told = w.step(t0 + Duration::from_secs(60), gone.clone());
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].event, NotifyEvent::ClientOffline);
    assert_eq!(told[0].title, "Client home is offline");
    assert!(!told[0].resolved);
    assert!(
        w.step(t0 + Duration::from_secs(600), gone).is_empty(),
        "no repeats"
    );
    let back = seen(&status(vec![client("home", true, None)]));
    let told = w.step(t0 + Duration::from_secs(660), back.clone());
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].event, NotifyEvent::ClientBack);
    assert!(told[0].resolved);
    assert_eq!(
        told[0].text,
        "Client \"home\" is connected again from 203.0.113.5:40000, after 11 min offline."
    );
    assert!(w.step(t0 + Duration::from_secs(700), back).is_empty());
}

#[test]
fn a_short_drop_sends_nothing_at_all() {
    let mut w = Watch::default();
    let t0 = Instant::now();
    let gone = seen(&status(vec![client("home", false, Some(1))]));
    assert!(w.step(t0, gone).is_empty());
    let back = seen(&status(vec![client("home", true, None)]));
    assert!(w.step(t0 + Duration::from_secs(20), back).is_empty());
}

#[test]
fn a_removed_client_is_forgotten_without_a_message() {
    let mut w = Watch::default();
    let t0 = Instant::now();
    let gone = seen(&status(vec![client("home", false, Some(1))]));
    w.step(t0, gone.clone());
    assert_eq!(w.step(t0 + Duration::from_secs(61), gone).len(), 1);
    assert!(
        w.step(t0 + Duration::from_secs(70), BTreeMap::new())
            .is_empty()
    );
    let back = seen(&status(vec![client("home", true, None)]));
    assert!(w.step(t0 + Duration::from_secs(80), back).is_empty());
}

fn service(health: Vec<(bool, Option<&str>)>) -> ServiceStatus {
    ServiceStatus {
        name: "wiki".into(),
        health: health
            .into_iter()
            .map(|(up, reason)| HealthStatus {
                client: "home".into(),
                target: None,
                up,
                reason: reason.map(str::to_string),
            })
            .collect(),
        ..ServiceStatus::default()
    }
}

#[test]
fn a_service_is_down_only_when_every_checked_client_fails() {
    let of = |s: ServiceStatus| {
        seen(&Status {
            services: vec![s],
            ..Status::default()
        })[&key(Kind::Service, "wiki")]
            .clone()
    };
    assert_eq!(of(service(vec![])), Health::Unknown);
    assert_eq!(
        of(service(vec![(false, Some("503")), (true, None)])),
        Health::Fine(String::new())
    );
    assert_eq!(
        of(service(vec![(false, None), (false, Some("refused"))])),
        Health::Problem("refused".into())
    );
    let mut w = Watch::default();
    let t0 = Instant::now();
    let down = BTreeMap::from([(key(Kind::Service, "wiki"), Health::Problem("503".into()))]);
    let told = w.step(t0, down);
    assert_eq!(told[0].event, NotifyEvent::SiteDown);
    assert_eq!(
        told[0].text,
        "Service \"wiki\" fails its health check on every client: 503."
    );
    let back = BTreeMap::from([(key(Kind::Service, "wiki"), Health::Fine(String::new()))]);
    assert_eq!(
        w.step(t0 + Duration::from_secs(5), back)[0].event,
        NotifyEvent::SiteBack
    );
}

#[test]
fn a_failing_certificate_is_told_and_its_fix_under_the_same_event() {
    let cert = |state| CertStatus {
        name: "wiki.example.com".into(),
        source: CertSourceStatus::LocalCa,
        check: None,
        state,
        expires: None,
        next_renewal: None,
        last_error: None,
        code: None,
        explanation: None,
        failures: 0,
    };
    let of = |state| {
        seen(&Status {
            certificates: vec![cert(state)],
            ..Status::default()
        })
    };
    let mut w = Watch::default();
    let t0 = Instant::now();
    let failing = of(CertStateStatus::Failing {
        reason: "rate limited".into(),
    });
    let told = w.step(t0, failing);
    assert_eq!(told[0].event, NotifyEvent::CertificateFailing);
    assert!(told[0].text.ends_with("rate limited."));
    assert!(
        w.step(t0, of(CertStateStatus::Renewing)).is_empty(),
        "a retry is not a fix"
    );
    let told = w.step(t0 + Duration::from_secs(120), of(CertStateStatus::Ok));
    assert_eq!(told[0].event, NotifyEvent::CertificateFailing);
    assert!(told[0].resolved);
}

fn country(stale: bool, failed: bool, built: Option<u64>) -> Status {
    Status {
        geoip: Some(burrow_admin_proto::status::GeoipStatus {
            source: "db-ip".into(),
            built,
            result: "download.db-ip.com answered 503".into(),
            failed,
            stale,
            ..Default::default()
        }),
        ..Status::default()
    }
}

#[test]
fn a_stale_country_database_is_told_once_and_once_when_fresh() {
    let mut w = Watch::default();
    let t0 = Instant::now();
    let fresh = seen(&country(false, false, Some(1)));
    assert!(w.step(t0, fresh.clone()).is_empty());
    let retrying = seen(&country(false, true, Some(1)));
    assert!(
        w.step(t0, retrying).is_empty(),
        "a failed download while the held copy is recent is not news"
    );
    let stale = seen(&country(true, true, Some(1)));
    let told = w.step(t0 + Duration::from_secs(60), stale.clone());
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].event, NotifyEvent::GeoipStale);
    assert_eq!(told[0].title, "The country database is out of date");
    assert!(told[0].text.contains("503"), "{}", told[0].text);
    assert!(
        w.step(t0 + Duration::from_secs(3600), stale).is_empty(),
        "no repeats"
    );
    let told = w.step(t0 + Duration::from_secs(7200), fresh);
    assert_eq!(told.len(), 1);
    assert!(told[0].resolved);
    assert_eq!(told[0].event, NotifyEvent::GeoipStale);
    assert_eq!(told[0].title, "The country database is up to date again");
}

#[test]
fn no_country_database_after_a_failure_is_a_problem_and_no_geoip_is_not_watched() {
    let none = seen(&country(false, true, None));
    assert!(matches!(
        none[&key(Kind::CountryDatabase, "db-ip")],
        Health::Problem(_)
    ));
    let starting = seen(&country(false, false, None));
    assert_eq!(
        starting[&key(Kind::CountryDatabase, "db-ip")],
        Health::Unknown
    );
    assert!(
        !seen(&Status::default())
            .keys()
            .any(|(k, _)| *k == Kind::CountryDatabase)
    );
}
