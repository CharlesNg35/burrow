use crate::cidr::{Cidr, Spans, within};

pub const CODES: [&str; 4] = [
    "admin-grant",
    "admin-upgrade",
    "admin-secret-added",
    "admin-from-widened",
];

pub fn is_grant_code(code: &str) -> bool {
    CODES.contains(&code)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Power<L, S> {
    pub level: Option<L>,
    pub secrets: Vec<S>,
    pub from: Vec<Cidr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Grant<'a, L, S> {
    Admin(L),
    Upgrade,
    SecretAdded(&'a S),
    FromWidened(Option<Cidr>),
}

impl<L, S> Grant<'_, L, S> {
    pub fn code(&self) -> &'static str {
        match self {
            Grant::Admin(_) => CODES[0],
            Grant::Upgrade => CODES[1],
            Grant::SecretAdded(_) => CODES[2],
            Grant::FromWidened(_) => CODES[3],
        }
    }
}

pub fn grants<'a, L: Ord + Copy, S: PartialEq>(
    old: Option<&Power<L, S>>,
    new: &'a Power<L, S>,
) -> Vec<Grant<'a, L, S>> {
    let Some(level) = new.level else {
        return Vec::new();
    };
    let Some(old) = old.filter(|o| o.level.is_some()) else {
        return vec![Grant::Admin(level)];
    };
    let mut out = Vec::new();
    if old.level < new.level {
        out.push(Grant::Upgrade);
    }
    out.extend(
        new.secrets
            .iter()
            .filter(|s| !old.secrets.contains(s))
            .map(Grant::SecretAdded),
    );
    if !within(&new.from, &old.from) {
        if new.from.is_empty() {
            out.push(Grant::FromWidened(None));
        } else {
            let spans = Spans::of(&old.from);
            out.extend(
                new.from
                    .iter()
                    .filter(|c| !spans.covers(c))
                    .map(|c| Grant::FromWidened(Some(*c))),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Level {
        Read,
        Write,
    }

    type P = Power<Level, &'static str>;

    fn power(level: Option<Level>, secrets: &[&'static str], from: &[&str]) -> P {
        Power {
            level,
            secrets: secrets.to_vec(),
            from: from.iter().map(|c| c.parse().unwrap()).collect(),
        }
    }

    fn codes(old: Option<&P>, new: &P) -> Vec<&'static str> {
        grants(old, new).iter().map(Grant::code).collect()
    }

    const R: Option<Level> = Some(Level::Read);
    const W: Option<Level> = Some(Level::Write);

    #[test]
    fn admin_for_a_visitor_without_it_is_a_grant() {
        let none = power(None, &["a"], &[]);
        assert_eq!(
            grants(Some(&none), &power(R, &["a"], &[])),
            vec![Grant::Admin(Level::Read)]
        );
        assert_eq!(
            grants(Some(&none), &power(W, &["a", "b"], &["10.0.0.0/8"])),
            vec![Grant::Admin(Level::Write)]
        );
        assert_eq!(
            grants(None, &power(W, &["a"], &[])),
            vec![Grant::Admin(Level::Write)]
        );
        assert!(grants(Some(&power(R, &["a"], &[])), &none).is_empty());
        assert!(grants(None, &power(None, &["a"], &[])).is_empty());
    }

    #[test]
    fn read_to_write_is_an_upgrade_and_write_to_read_is_not() {
        assert_eq!(
            codes(Some(&power(R, &["a"], &[])), &power(W, &["a"], &[])),
            ["admin-upgrade"]
        );
        assert!(codes(Some(&power(W, &["a"], &[])), &power(R, &["a"], &[])).is_empty());
        assert!(codes(Some(&power(W, &["a"], &[])), &power(W, &["a"], &[])).is_empty());
        assert!(codes(Some(&power(R, &["a"], &[])), &power(R, &["a"], &[])).is_empty());
    }

    #[test]
    fn a_new_secret_on_an_admin_is_a_grant_and_a_removed_one_is_not() {
        let old = power(W, &["a", "b"], &[]);
        assert_eq!(
            grants(Some(&old), &power(W, &["a", "b", "c"], &[])),
            vec![Grant::SecretAdded(&"c")]
        );
        assert_eq!(
            grants(Some(&old), &power(W, &["a", "c"], &[])),
            vec![Grant::SecretAdded(&"c")]
        );
        assert!(grants(Some(&old), &power(W, &["b"], &[])).is_empty());
        assert!(grants(Some(&old), &power(W, &["b", "a"], &[])).is_empty());
        assert!(grants(Some(&old), &power(None, &["c"], &[])).is_empty());
        assert!(
            grants(
                Some(&power(None, &["a"], &[])),
                &power(None, &["a", "c"], &[])
            )
            .is_empty()
        );
    }

    #[test]
    fn admin_from_may_narrow_but_not_widen() {
        let old = power(W, &["a"], &["10.0.0.0/8", "2001:db8::/32"]);
        let same = |from: &[&str]| codes(Some(&old), &power(W, &["a"], from));
        assert!(same(&["10.0.0.0/8"]).is_empty());
        assert!(same(&["10.1.0.0/16", "10.2.0.0/16", "2001:db8:1::/48"]).is_empty());
        assert!(same(&["10.9.8.7", "::ffff:10.1.2.3"]).is_empty());
        assert_eq!(same(&["10.0.0.0/8", "192.0.2.1"]), ["admin-from-widened"]);
        assert_eq!(same(&["10.0.0.0/7"]), ["admin-from-widened"]);
        assert_eq!(same(&["::/0"]), ["admin-from-widened"]);
        assert_eq!(same(&["10.0.0.0/8", "2001:db9::1"]), ["admin-from-widened"]);
        assert_eq!(
            grants(Some(&old), &power(W, &["a"], &[])),
            vec![Grant::FromWidened(None)]
        );
        assert_eq!(
            grants(
                Some(&old),
                &power(W, &["a"], &["10.0.0.0/8", "192.0.2.0/24"])
            ),
            vec![Grant::FromWidened(Some("192.0.2.0/24".parse().unwrap()))]
        );
        let split = power(W, &["a"], &["10.0.0.0/25", "10.0.0.128/25"]);
        assert!(codes(Some(&split), &power(W, &["a"], &["10.0.0.0/24"])).is_empty());
        let everywhere = power(W, &["a"], &[]);
        assert!(codes(Some(&everywhere), &power(W, &["a"], &["0.0.0.0/0", "::/0"])).is_empty());
        assert!(codes(Some(&everywhere), &power(W, &["a"], &["203.0.113.9"])).is_empty());
        let both = power(W, &["a"], &["0.0.0.0/0", "::/0"]);
        assert!(codes(Some(&both), &power(W, &["a"], &[])).is_empty());
        assert!(codes(Some(&old), &power(None, &["a"], &[])).is_empty());
    }

    #[test]
    fn every_kind_is_reported_once_per_cause() {
        let old = power(R, &["a"], &["10.0.0.0/8"]);
        assert_eq!(
            codes(
                Some(&old),
                &power(W, &["b"], &["192.0.2.1", "198.51.100.0/24"])
            ),
            [
                "admin-upgrade",
                "admin-secret-added",
                "admin-from-widened",
                "admin-from-widened"
            ]
        );
        assert!(CODES.iter().all(|c| is_grant_code(c)));
        assert!(!is_grant_code("self-demoted"));
    }
}
