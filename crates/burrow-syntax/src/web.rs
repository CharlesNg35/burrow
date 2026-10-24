use std::net::IpAddr;

pub const MAX_HOST_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleLabel {
    Allowed,
    Refused,
}

pub fn host_name(text: &str, single: SingleLabel) -> Result<String, String> {
    let t = text.trim();
    let bare = t
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(t);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(ip.to_canonical().to_string());
    }
    if t.is_empty() {
        return Err("a host name is required, like blog.example.com".into());
    }
    if t.contains("://") || t.contains('/') || t.contains(':') || t.contains(' ') {
        return Err(format!(
            "\"{text}\" is not a host name; write the name alone, like blog.example.com"
        ));
    }
    let name = t.strip_suffix('.').unwrap_or(t).to_ascii_lowercase();
    let labels: Vec<&str> = name.split('.').collect();
    let wildcard = labels[0] == "*";
    if labels.iter().skip(1).any(|l| l.contains('*')) || (labels[0].contains('*') && !wildcard) {
        return Err(format!(
            "\"{text}\" is not a valid wildcard; a wildcard is \"*.\" followed by a name, like *.example.com"
        ));
    }
    if wildcard && labels.len() < 3 {
        return Err(format!(
            "\"{text}\" is too wide; a wildcard needs a parent name, like *.example.com"
        ));
    }
    if labels.len() < 2 && single == SingleLabel::Refused {
        return Err(format!(
            "\"{text}\" is not a host name; write a full name, like blog.example.com"
        ));
    }
    let named = &labels[usize::from(wildcard)..];
    if name.len() > MAX_HOST_LEN || !named.iter().all(|l| good_label(l)) {
        return Err(format!(
            "\"{text}\" is not a host name; use letters, digits and - between dots, like blog.example.com"
        ));
    }
    if labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!(
            "\"{text}\" is not a host name or an IP address; write a name like blog.example.com or an address like 203.0.113.10"
        ));
    }
    Ok(name)
}

fn good_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= MAX_LABEL_LEN
        && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !l.starts_with('-')
        && !l.ends_with('-')
}

pub fn is_wildcard(host: &str) -> bool {
    host.starts_with("*.")
}

pub fn is_ip(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
}

pub fn covers(pattern: &str, name: &str) -> bool {
    if pattern == name {
        return true;
    }
    match (pattern.strip_prefix("*."), name.split_once('.')) {
        (Some(parent), Some((first, rest))) => !first.is_empty() && first != "*" && rest == parent,
        _ => false,
    }
}

pub fn wildcard_of(name: &str) -> Option<String> {
    let (first, rest) = name.split_once('.')?;
    (!first.is_empty() && rest.contains('.')).then(|| format!("*.{rest}"))
}

pub fn sni_name(raw: &str) -> String {
    raw.strip_suffix('.').unwrap_or(raw).to_ascii_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Twice {
    pub first: usize,
    pub second: usize,
}

pub fn entry_for<'a>(
    entries: impl IntoIterator<Item = (usize, &'a [String])>,
    name: &str,
) -> Result<Option<usize>, Twice> {
    let name = sni_name(name.trim());
    let (mut exact, mut wild): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    for (i, hosts) in entries {
        let listed = hosts.iter().map(|h| sni_name(h.trim()));
        let mut kind = None;
        for h in listed {
            if h == name {
                kind = Some(true);
                break;
            }
            if covers(&h, &name) {
                kind = Some(false);
            }
        }
        match kind {
            Some(true) => exact.push(i),
            Some(false) => wild.push(i),
            None => {}
        }
    }
    let pick = if exact.is_empty() { wild } else { exact };
    match pick.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        [first, second, ..] => Err(Twice {
            first: *first,
            second: *second,
        }),
    }
}

mod check;

pub use check::{
    AutoName, Challenge, Check, CheckPorts, HTTP_PORT, NoCheck, PROXY_NOTE, TLS_PORT, checks_for,
    proxy_note,
};

#[cfg(test)]
mod tests;
