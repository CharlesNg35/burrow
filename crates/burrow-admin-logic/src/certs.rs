use burrow_admin_proto::codes::PROFILE_NOT_OFFERED;
pub use burrow_admin_proto::date::date;
use burrow_admin_proto::status::{
    CertCheck, CertSourceStatus, CertStateStatus, CertStatus, Status,
};
use burrow_config::{LetsEncrypt, ServerConfig, WebConfig};
use burrow_syntax::duration::{DAY_SECS as DAY, span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    Info,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub source: String,
    pub check: String,
    pub state: (String, Level),
    pub expires: (String, Level),
    pub expires_on: String,
    pub next: String,
    pub problem: Option<String>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub key: String,
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub service: Option<String>,
    pub field: Option<Vec<String>>,
}

fn source(c: &CertStatus) -> String {
    match &c.source {
        CertSourceStatus::OwnFile { path } => format!("File {path}"),
        CertSourceStatus::SharedFile { path } => format!("File {path} (covers several names)"),
        CertSourceStatus::TunnelFile { path } => format!("File of the WebSocket listener {path}"),
        CertSourceStatus::LetsEncrypt { directory }
            if directory == burrow_syntax::web::LETSENCRYPT_STAGING =>
        {
            "Let's Encrypt (test servers)".into()
        }
        CertSourceStatus::LetsEncrypt { .. } => "Let's Encrypt".into(),
        CertSourceStatus::LocalCa => "burrow's local CA".into(),
        CertSourceStatus::Passthrough => {
            crate::wording::capital(&burrow_config::labels::app_https())
        }
        CertSourceStatus::Plain => "None: plain HTTP".into(),
    }
}

const YEAR: u64 = 365 * DAY;
pub fn ends(at: u64, now: u64) -> String {
    match at {
        t if t <= now => format!("expired {} ago", span(now - t)),
        t if t - now >= YEAR => date(t),
        t => format!("in {}", span(t - now)),
    }
}

fn expires(c: &CertStatus, now: u64) -> (String, Level) {
    match c.expires {
        None => ("—".into(), Level::Info),
        Some(t) if t <= now => (ends(t, now), Level::Bad),
        Some(t) if burrow_syntax::web::expiry_due(t, c.next_renewal, c.failures, now) => {
            (ends(t, now), Level::Warn)
        }
        Some(t) => (ends(t, now), Level::Good),
    }
}

fn next(c: &CertStatus, now: u64) -> String {
    let Some(t) = c.next_renewal else {
        return "—".into();
    };
    let at = span(t.saturating_sub(now));
    match c.state {
        CertStateStatus::Failing { .. } | CertStateStatus::Pending => format!("next try in {at}"),
        _ => format!("in {at}"),
    }
}

fn burrows(c: &&CertStatus) -> bool {
    !matches!(
        c.source,
        CertSourceStatus::Passthrough | CertSourceStatus::Plain
    )
}

pub fn rows(status: &Status, now: u64) -> Vec<Row> {
    status
        .certificates
        .iter()
        .filter(burrows)
        .map(|c| (c, expires(c, now)))
        .map(|(c, ends)| Row {
            name: c.name.clone(),
            source: source(c),
            check: match c.check {
                Some(CertCheck::Tls) => "TLS".into(),
                Some(CertCheck::Http) => "HTTP".into(),
                Some(CertCheck::Dns) => "DNS".into(),
                None => "—".into(),
            },
            state: match (&c.state, ends.1) {
                (CertStateStatus::Ok, Level::Warn) => ("Expiring soon".into(), Level::Warn),
                (CertStateStatus::Ok, Level::Bad) => ("Expired".into(), Level::Bad),
                (CertStateStatus::Ok, _) => ("Valid".into(), Level::Good),
                (CertStateStatus::Pending, _) => ("Requesting a certificate".into(), Level::Info),
                (CertStateStatus::Renewing, _) => ("Renewing".into(), Level::Info),
                (CertStateStatus::Failing { .. }, _) => ("Failing".into(), Level::Bad),
            },
            expires: ends,
            expires_on: c.expires.map(date).unwrap_or_default(),
            next: next(c, now),
            problem: match &c.state {
                CertStateStatus::Failing { reason } => Some(reason.clone()),
                _ => c.last_error.clone(),
            },
            explanation: c.explanation.clone(),
        })
        .collect()
}

pub fn count(status: &Status, now: u64) -> String {
    let all = rows(status, now);
    let expiring = all
        .iter()
        .filter(|r| matches!(r.expires.1, Level::Warn | Level::Bad))
        .count();
    let failing = all
        .iter()
        .filter(|r| r.state.1 == Level::Bad && r.expires.1 != Level::Bad)
        .count();
    let mut out = all.len().to_string();
    if expiring > 0 {
        out.push_str(&format!(" · {expiring} expiring"));
    }
    if failing > 0 {
        out.push_str(&format!(" · {failing} failing"));
    }
    out
}

fn service_of(status: &Status, name: &str) -> Option<String> {
    let web = status.web.as_ref()?;
    web.hosts
        .iter()
        .find(|h| h.name == name)
        .and_then(|h| h.first_service().map(str::to_string))
}

pub fn site_badge(status: &Status, now: u64, service: &str) -> Option<(String, Level)> {
    status
        .certificates
        .iter()
        .filter(burrows)
        .filter(|c| service_of(status, &c.name).as_deref() == Some(service))
        .find_map(|c| match (&c.state, c.expires) {
            (CertStateStatus::Failing { .. }, _) => {
                Some(("certificate failing".into(), Level::Bad))
            }
            (_, Some(t)) if t <= now => Some(("certificate expired".into(), Level::Bad)),
            (CertStateStatus::Ok, Some(t))
                if burrow_syntax::web::expiry_due(t, c.next_renewal, c.failures, now) =>
            {
                Some((format!("expires in {}", span(t - now)), Level::Warn))
            }
            _ => None,
        })
}

pub fn notices(status: &Status, now: u64) -> Vec<Notice> {
    let mut out: Vec<Notice> = Vec::new();
    let mut names: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for c in status.certificates.iter().filter(burrows) {
        let (subject, key) = match &c.source {
            CertSourceStatus::SharedFile { path } => {
                (format!("file {path}"), format!("cert-file:{path}"))
            }
            _ => (format!("for {}", c.name), format!("cert:{}", c.name)),
        };
        if let Some(n) = names.get_mut(&key) {
            *n += 1;
            continue;
        }
        let service = service_of(status, &c.name);
        let field = (c.code.as_deref() == Some(PROFILE_NOT_OFFERED)).then(|| {
            [
                ServerConfig::WEB.key(),
                WebConfig::LETSENCRYPT.key(),
                LetsEncrypt::PROFILE.key(),
            ]
            .map(str::to_string)
            .to_vec()
        });
        let notice = |level, title: String, detail: String| Notice {
            key: key.clone(),
            level,
            title,
            detail,
            service: service.clone(),
            field: field.clone(),
        };
        if let CertStateStatus::Failing { reason } = &c.state {
            let tries = match c.failures {
                0 | 1 => String::new(),
                n => format!(" · tried {n} times"),
            };
            names.insert(key.clone(), 1);
            out.push(notice(
                Level::Bad,
                format!("The certificate {subject} cannot be renewed"),
                format!("{reason}{tries}"),
            ));
            continue;
        }
        let before = out.len();
        match c.expires {
            Some(t) if t <= now => out.push(notice(
                Level::Bad,
                format!("The certificate {subject} has expired"),
                "Browsers refuse the site until it is renewed.".into(),
            )),
            Some(t)
                if burrow_syntax::web::expiry_due(t, c.next_renewal, c.failures, now)
                    && c.state == CertStateStatus::Ok =>
            {
                out.push(notice(
                    Level::Warn,
                    format!("The certificate {subject} expires in {}", span(t - now)),
                    "Replace the file, or let Let's Encrypt renew it.".into(),
                ))
            }
            _ => {}
        }
        if out.len() > before {
            names.insert(key, 1);
        }
    }
    for n in &mut out {
        if let Some(&k) = names.get(&n.key).filter(|k| **k > 1) {
            n.detail = format!("It covers {k} names. {}", n.detail);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn cert(name: &str, state: CertStateStatus, expires: Option<u64>) -> CertStatus {
        CertStatus {
            name: name.into(),
            source: CertSourceStatus::LetsEncrypt {
                directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
            },
            check: Some(CertCheck::Tls),
            state,
            expires,
            next_renewal: Some(NOW + 600),
            last_error: None,
            code: None,
            explanation: None,
            failures: 0,
        }
    }

    fn status(certs: Vec<CertStatus>) -> Status {
        Status {
            certificates: certs,
            web: Some(burrow_admin_proto::web::WebSummary {
                hosts: vec![burrow_admin_proto::web::HostLine {
                    name: "blog.example.com".into(),
                    service: "blog".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Status::default()
        }
    }

    #[test]
    fn a_valid_certificate_near_its_end_reads_as_expiring_not_valid() {
        let soon = CertStatus {
            next_renewal: None,
            ..cert("old.example.com", CertStateStatus::Ok, Some(NOW + 5 * DAY))
        };
        let gone = cert("gone.example.com", CertStateStatus::Ok, Some(NOW - DAY));
        let r = rows(&status(vec![soon, gone]), NOW);
        assert_eq!(r[0].state, ("Expiring soon".to_string(), Level::Warn));
        assert_eq!(r[1].state, ("Expired".to_string(), Level::Bad));
    }

    #[test]
    fn a_row_says_where_a_certificate_comes_from_and_when_it_ends() {
        let s = status(vec![
            cert(
                "blog.example.com",
                CertStateStatus::Ok,
                Some(NOW + 60 * DAY),
            ),
            CertStatus {
                failures: 3,
                ..cert(
                    "wiki.example.com",
                    CertStateStatus::Failing {
                        reason: "the DNS name does not exist".into(),
                    },
                    None,
                )
            },
        ]);
        let mut with_app = s.clone();
        with_app.certificates.push(CertStatus {
            source: CertSourceStatus::Passthrough,
            ..cert("nas.example.com", CertStateStatus::Ok, None)
        });
        assert_eq!(
            rows(&with_app, NOW).len(),
            2,
            "the app's own certificates are not burrow's"
        );
        let r = rows(&s, NOW);
        assert_eq!(r[0].source, "Let's Encrypt");
        assert_eq!(r[0].check, "TLS");
        assert_eq!(r[0].state, ("Valid".to_string(), Level::Good));
        assert_eq!(r[0].expires, ("in 60d".to_string(), Level::Good));
        assert_eq!(r[0].next, "in 10m");
        assert_eq!(r[0].expires_on, date(NOW + 60 * DAY));
        assert_eq!(ends(NOW + 400 * DAY, NOW), date(NOW + 400 * DAY));
        assert_eq!(ends(NOW + 20 * DAY, NOW), "in 20d");
        assert_eq!(r[1].state.1, Level::Bad);
        assert_eq!(r[1].next, "next try in 10m");
        assert_eq!(r[1].problem.as_deref(), Some("the DNS name does not exist"));
    }

    #[test]
    fn the_count_says_how_many_certificates_expire_soon_or_fail() {
        let ok = cert("a.example.com", CertStateStatus::Ok, Some(NOW + 60 * DAY));
        assert_eq!(count(&status(vec![ok.clone()]), NOW), "1");
        let soon = CertStatus {
            next_renewal: Some(NOW - DAY),
            ..cert("b.example.com", CertStateStatus::Ok, Some(NOW + 5 * DAY))
        };
        let failing = cert(
            "c.example.com",
            CertStateStatus::Failing {
                reason: "rate limited".into(),
            },
            Some(NOW + 60 * DAY),
        );
        let app = CertStatus {
            source: CertSourceStatus::Passthrough,
            ..cert("d.example.com", CertStateStatus::Ok, None)
        };
        assert_eq!(
            count(&status(vec![ok, soon, failing, app]), NOW),
            "3 · 1 expiring · 1 failing"
        );
        assert_eq!(count(&status(Vec::new()), NOW), "0");
    }

    #[test]
    fn failing_expired_and_soon_expiring_certificates_need_attention() {
        let s = status(vec![
            CertStatus {
                failures: 3,
                ..cert(
                    "blog.example.com",
                    CertStateStatus::Failing {
                        reason: "rate limited".into(),
                    },
                    Some(NOW + 20 * DAY),
                )
            },
            cert("old.example.com", CertStateStatus::Ok, Some(NOW - DAY)),
            CertStatus {
                next_renewal: Some(NOW - DAY),
                ..cert("soon.example.com", CertStateStatus::Ok, Some(NOW + 3 * DAY))
            },
            cert(
                "fine.example.com",
                CertStateStatus::Ok,
                Some(NOW + 40 * DAY),
            ),
            cert(
                "busy.example.com",
                CertStateStatus::Renewing,
                Some(NOW + 3 * DAY),
            ),
        ]);
        let n = notices(&s, NOW);
        let titles: Vec<&str> = n.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "The certificate for blog.example.com cannot be renewed",
                "The certificate for old.example.com has expired",
                "The certificate for soon.example.com expires in 3d",
            ]
        );
        assert_eq!(n[0].detail, "rate limited · tried 3 times");
        assert_eq!(n[0].service.as_deref(), Some("blog"));
        assert_eq!(n[1].service, None);
        assert_eq!(n[0].field, None);
        let mut p = s.clone();
        p.certificates[0].code = Some(PROFILE_NOT_OFFERED.into());
        assert_eq!(
            notices(&p, NOW)[0].field.as_deref(),
            Some(&["web".to_string(), "letsencrypt".into(), "profile".into()][..]),
            "a profile the service does not offer links to the field"
        );
    }

    #[test]
    fn one_shared_file_near_its_end_is_one_notice_not_one_per_name() {
        let shared = |name: &str| CertStatus {
            source: CertSourceStatus::SharedFile {
                path: "./portal.pem".into(),
            },
            check: None,
            next_renewal: None,
            ..cert(name, CertStateStatus::Ok, Some(NOW + 9 * DAY))
        };
        let n = notices(
            &status(vec![
                shared("portal.example.com"),
                shared("www.portal.example.com"),
                shared("old.example.com"),
            ]),
            NOW,
        );
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(
            n[0].title,
            "The certificate file ./portal.pem expires in 9d"
        );
        assert_eq!(
            n[0].detail,
            "It covers 3 names. Replace the file, or let Let's Encrypt renew it."
        );
    }

    #[test]
    fn a_site_shows_its_own_certificate_problem_in_place() {
        let s = status(vec![CertStatus {
            next_renewal: None,
            ..cert("blog.example.com", CertStateStatus::Ok, Some(NOW + 9 * DAY))
        }]);
        assert_eq!(
            site_badge(&s, NOW, "blog"),
            Some(("expires in 9d".to_string(), Level::Warn))
        );
        assert_eq!(site_badge(&s, NOW, "other"), None);
        let fine = status(vec![cert(
            "blog.example.com",
            CertStateStatus::Ok,
            Some(NOW + 60 * DAY),
        )]);
        assert_eq!(site_badge(&fine, NOW, "blog"), None);
        let bad = status(vec![cert(
            "blog.example.com",
            CertStateStatus::Failing { reason: "x".into() },
            None,
        )]);
        assert_eq!(
            site_badge(&bad, NOW, "blog"),
            Some(("certificate failing".to_string(), Level::Bad))
        );
    }

    #[test]
    fn a_short_lived_certificate_burrow_renews_on_time_needs_no_attention() {
        let hour = DAY / 24;
        let local = CertStatus {
            source: CertSourceStatus::LocalCa,
            check: None,
            next_renewal: Some(NOW + hour),
            ..cert("grafana.lan", CertStateStatus::Ok, Some(NOW + 11 * hour))
        };
        assert!(notices(&status(vec![local.clone()]), NOW).is_empty());
        assert_eq!(count(&status(vec![local.clone()]), NOW), "1");
        let late = CertStatus {
            next_renewal: Some(NOW - hour),
            ..local
        };
        assert_eq!(count(&status(vec![late]), NOW), "1 · 1 expiring");
    }
}
