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
                (Some((u.to_string(), Zeroizing::new(p.to_string()))), h)
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
}
