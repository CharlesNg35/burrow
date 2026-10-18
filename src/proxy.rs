use std::fmt;

use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Eq)]
pub enum ProxyKind {
    Http,
    Socks5,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub addr: String,
    pub auth: Option<(String, Zeroizing<String>)>,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Socks5 => "socks5",
        };
        write!(f, "{scheme}://{}", self.addr)
    }
}

impl ProxyConfig {
    pub fn parse(url: &str) -> Result<Self, String> {
        let (kind, rest) = if let Some(r) = url.strip_prefix("http://") {
            (ProxyKind::Http, r)
        } else if let Some(r) = url
            .strip_prefix("socks5://")
            .or_else(|| url.strip_prefix("socks5h://"))
        {
            (ProxyKind::Socks5, r)
        } else {
            return Err("proxy must start with http:// or socks5://".into());
        };
        let rest = rest.trim_end_matches('/');
        let (auth, hostport) = match rest.rsplit_once('@') {
            Some((a, h)) => {
                let (u, p) = a.split_once(':').unwrap_or((a, ""));
                let p = Zeroizing::new(decode(p)?);
                (Some((decode(u)?, p)), h)
            }
            None => (None, rest),
        };
        if let Some((u, p)) = &auth
            && (u.len() > 255 || p.len() > 255)
        {
            return Err("proxy user name and password must be at most 255 bytes".into());
        }
        crate::config::check::parse_host_port(hostport)?;
        Ok(ProxyConfig {
            kind,
            addr: hostport.to_string(),
            auth,
        })
    }
}

fn decode(text: &str) -> Result<String, String> {
    let bad = || {
        "proxy user name and password may use %XX escapes only for bytes, such as %40 for @"
            .to_string()
    };
    let bytes = text.as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3).ok_or_else(bad)?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| bad())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out.to_vec())
        .map_err(|_| "proxy user name and password must be UTF-8 text".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let p = ProxyConfig::parse("socks5://u:pw@proxy.local:1080").unwrap();
        assert!(p.kind == ProxyKind::Socks5);
        assert_eq!(p.addr, "proxy.local:1080");
        assert_eq!(p.auth.as_ref().unwrap().0, "u");
        assert_eq!(format!("{p:?}"), "socks5://proxy.local:1080");
        assert!(ProxyConfig::parse("http://10.0.0.1:3128/").is_ok());
        assert!(ProxyConfig::parse("ftp://x:1").is_err());
        assert!(ProxyConfig::parse("http://nohost").is_err());
    }

    #[test]
    fn credentials_are_percent_decoded() {
        let p = ProxyConfig::parse("http://us%3Aer:p%40ss%2Fw@proxy.local:3128").unwrap();
        let (user, pass) = p.auth.unwrap();
        assert_eq!(user, "us:er");
        assert_eq!(pass.as_str(), "p@ss/w");
        assert!(ProxyConfig::parse("http://u:p%4@proxy.local:3128").is_err());
        assert!(ProxyConfig::parse("http://u:p%zz@proxy.local:3128").is_err());
        assert!(ProxyConfig::parse("http://u:%ff@proxy.local:3128").is_err());
        let plain = ProxyConfig::parse("http://u:p+w@proxy.local:3128").unwrap();
        assert_eq!(plain.auth.unwrap().1.as_str(), "p+w");
    }
}
