use std::net::IpAddr;

use crate::hostport::{MAX_TARGET_LEN, parse_host_port};
use crate::ports::{MAX_RANGE, parse_range, parse_ranged_text, show};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetValue {
    Net {
        host: String,
        first: u16,
        count: u16,
    },
    Unix(String),
}

impl TargetValue {
    pub fn count(&self) -> u16 {
        match self {
            TargetValue::Net { count, .. } => *count,
            TargetValue::Unix(_) => 1,
        }
    }

    pub fn short(&self) -> String {
        match self {
            TargetValue::Net { host, first, count } if host == "127.0.0.1" => show(*first, *count),
            other => other.full(),
        }
    }

    pub fn full(&self) -> String {
        match self {
            TargetValue::Net { host, first, count } => {
                format!(
                    "{}:{}",
                    crate::address::url_host(host),
                    show(*first, *count)
                )
            }
            TargetValue::Unix(p) => format!("unix:{p}"),
        }
    }
}

pub fn parse_target(text: &str) -> Result<TargetValue, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("a target is required".into());
    }
    if let Some(path) = crate::unix::path_of(t) {
        crate::unix::check_path(path)?;
        return Ok(TargetValue::Unix(path.to_string()));
    }
    if t.len() > MAX_TARGET_LEN {
        return Err(format!("a target has at most {MAX_TARGET_LEN} characters"));
    }
    if t.chars().all(|c| c.is_ascii_digit() || c == '-') {
        let (first, count) = parse_range(t)?;
        return Ok(TargetValue::Net {
            host: "127.0.0.1".into(),
            first,
            count,
        });
    }
    let (host_part, ports) = match t.rsplit_once(':') {
        Some((h, p)) if p.contains('-') => (h, Some(p)),
        _ => (t, None),
    };
    match ports {
        Some(p) => {
            let (first, count) = parse_range(p)?;
            let (host, _) = parse_host_port(&format!("{host_part}:{first}"))?;
            Ok(TargetValue::Net { host, first, count })
        }
        None => {
            let (host, first) = parse_host_port(t)?;
            Ok(TargetValue::Net {
                host,
                first,
                count: 1,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindValue {
    pub ip: IpAddr,
    pub first: u16,
    pub count: u16,
}

impl BindValue {
    pub fn short(&self, default_ip: IpAddr) -> String {
        if self.ip == default_ip {
            show(self.first, self.count)
        } else {
            crate::ip::show_with_port(self.ip, &show(self.first, self.count))
        }
    }
}

pub fn parse_bind(text: &str, default_ip: [u8; 4], max_range: u16) -> Result<BindValue, String> {
    let (addr, count) = parse_ranged_text(text, default_ip, "bind")?;
    if count > max_range.min(MAX_RANGE) {
        return Err(format!("at most {} ports", max_range.min(MAX_RANGE)));
    }
    Ok(BindValue {
        ip: addr.ip(),
        first: addr.port(),
        count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(
            parse_target("3000").unwrap(),
            TargetValue::Net {
                host: "127.0.0.1".into(),
                first: 3000,
                count: 1
            }
        );
        assert_eq!(parse_target("3000-3002").unwrap().count(), 3);
        assert_eq!(parse_target("db:5432").unwrap().full(), "db:5432");
        assert_eq!(parse_target("[::1]:80-81").unwrap().full(), "[::1]:80-81");
        assert_eq!(
            parse_target("unix:/run/app.sock").unwrap(),
            TargetValue::Unix("/run/app.sock".into())
        );
        assert!(parse_target("unix:run").is_err());
        assert!(parse_target("::1:80").is_err());
        assert!(parse_target("h:9-1").is_err());
        assert_eq!(parse_target("127.0.0.1:22").unwrap().short(), "22");
    }

    #[test]
    fn binds() {
        let b = parse_bind("0.0.0.0:30000-30010", [0, 0, 0, 0], 1000).unwrap();
        assert_eq!(b.count, 11);
        assert_eq!(b.short("0.0.0.0".parse().unwrap()), "30000-30010");
        let v6 = parse_bind("[::1]:22", [0, 0, 0, 0], 1).unwrap();
        assert_eq!(v6.short("0.0.0.0".parse().unwrap()), "[::1]:22");
        assert!(parse_bind("1-20", [0, 0, 0, 0], 10).is_err());
    }
}
