pub use burrow_admin_proto::date::date;
use burrow_admin_proto::status::{
    CertCheck, CertSourceStatus, CertStateStatus, CertStatus, Status,
};
use burrow_config::{LetsEncrypt, ServerConfig, WebConfig};

const DAY: u64 = 86_400;
const SOON: u64 = 14 * DAY;

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

pub const PROFILE_NOT_OFFERED: &str = "profile-not-offered";

fn days(secs: u64) -> String {
    match secs / DAY {
        d if d >= 730 => format!("{} years", d / 365),
        0 => match secs / 3600 {
            0 => format!("{} min", (secs / 60).max(1)),
            1 => "1 hour".into(),
            h => format!("{h} hours"),
        },
        1 => "1 day".into(),
        d => format!("{d} days"),
    }
}

fn source(c: &CertStatus) -> String {
    match &c.source {
        CertSourceStatus::OwnFile { path } => format!("Own file {path}"),
        CertSourceStatus::SharedFile { path } => format!("Shared file {path}"),
        CertSourceStatus::TunnelFile { path } => format!("The tunnel's file {path}"),
        CertSourceStatus::LetsEncrypt { directory }
            if directory == burrow_syntax::web::LETSENCRYPT_STAGING =>
        {
            "Let's Encrypt (test service)".into()
        }
        CertSourceStatus::LetsEncrypt { .. } => "Let's Encrypt".into(),
        CertSourceStatus::LocalCa => "burrow's local CA".into(),
        CertSourceStatus::Passthrough => "The app's own".into(),
        CertSourceStatus::Plain => "None: plain HTTP".into(),
    }
}

const YEAR: u64 = 365 * DAY;
pub fn ends(at: u64, now: u64) -> String {
    match at {
        t if t <= now => format!("expired {} ago", days(now - t)),
        t if t - now >= YEAR => date(t),
        t => format!("in {}", days(t - now)),
    }
}

fn expires(c: &CertStatus, now: u64) -> (String, Level) {
    match c.expires {
        None => ("—".into(), Level::Info),
        Some(t) if t <= now => (ends(t, now), Level::Bad),
        Some(t) if t - now < SOON => (ends(t, now), Level::Warn),
        Some(t) => (ends(t, now), Level::Good),
    }
}

fn next(c: &CertStatus, now: u64) -> String {
    let Some(t) = c.next_renewal else {
        return "—".into();
    };
    let at = days(t.saturating_sub(now));
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
        .map(|c| Row {
            name: c.name.clone(),
            source: source(c),
            check: match c.check {
                Some(CertCheck::Tls) => "TLS".into(),
                Some(CertCheck::Http) => "HTTP".into(),
                Some(CertCheck::Dns) => "DNS".into(),
                None => "—".into(),
            },
            state: match &c.state {
                CertStateStatus::Ok => ("Valid".into(), Level::Good),
                CertStateStatus::Pending => ("Getting it".into(), Level::Info),
                CertStateStatus::Renewing => ("Renewing".into(), Level::Info),
                CertStateStatus::Failing { .. } => ("Failing".into(), Level::Bad),
            },
            expires: expires(c, now),
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

fn service_of(status: &Status, name: &str) -> Option<String> {
    let web = status.web.as_ref()?;
    web.hosts
        .iter()
        .find(|h| h.name == name)
        .and_then(|h| h.first_service().map(str::to_string))
}

pub fn notices(status: &Status, now: u64) -> Vec<Notice> {
    let mut out = Vec::new();
    for c in status.certificates.iter().filter(burrows) {
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
            key: format!("cert:{}", c.name),
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
            out.push(notice(
                Level::Bad,
                format!("The certificate for {} cannot be renewed", c.name),
                format!("{reason}{tries}"),
            ));
            continue;
        }
        match c.expires {
            Some(t) if t <= now => out.push(notice(
                Level::Bad,
                format!("The certificate for {} has expired", c.name),
                "Browsers refuse the site until it is renewed.".into(),
            )),
            Some(t) if t - now < SOON && c.state == CertStateStatus::Ok => out.push(notice(
                Level::Warn,
                format!(
                    "The certificate for {} expires in {}",
                    c.name,
                    days(t - now)
                ),
                "Replace the file, or let Let's Encrypt renew it.".into(),
            )),
            _ => {}
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
        assert_eq!(r[0].expires, ("in 60 days".to_string(), Level::Good));
        assert_eq!(r[0].next, "in 10 min");
        assert_eq!(days(800 * DAY), "2 years");
        assert_eq!(r[0].expires_on, date(NOW + 60 * DAY));
        assert_eq!(ends(NOW + 400 * DAY, NOW), date(NOW + 400 * DAY));
        assert_eq!(ends(NOW + 20 * DAY, NOW), "in 20 days");
        assert_eq!(r[1].state.1, Level::Bad);
        assert_eq!(r[1].next, "next try in 10 min");
        assert_eq!(r[1].problem.as_deref(), Some("the DNS name does not exist"));
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
            cert("soon.example.com", CertStateStatus::Ok, Some(NOW + 3 * DAY)),
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
                "The certificate for soon.example.com expires in 3 days",
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
}
