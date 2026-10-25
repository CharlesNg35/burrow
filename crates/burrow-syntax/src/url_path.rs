use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    NoSlash,
    BadEscape,
    Nul,
    Backslash,
    DotSegment,
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn kept_raw(b: u8) -> bool {
    matches!(b, 0x21 | 0x24..=0x3B | 0x3D | 0x40..=0x5B | 0x5D..=0x5F | 0x61..=0x7E)
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn push_escape(out: &mut String, b: u8) {
    out.push('%');
    out.push(char::from(HEX[usize::from(b >> 4)]));
    out.push(char::from(HEX[usize::from(b & 15)]));
}

fn push_byte(out: &mut String, b: u8) {
    if b == b'/' && out.ends_with('/') {
        return;
    }
    if kept_raw(b) {
        out.push(char::from(b));
    } else {
        push_escape(out, b);
    }
}

pub fn normalize_path(raw: &str) -> Result<String, PathError> {
    let bytes = raw.as_bytes();
    if bytes.first() != Some(&b'/') {
        return Err(PathError::NoSlash);
    }
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'%' => {
                let (Some(hi), Some(lo)) = (
                    bytes.get(i + 1).copied().and_then(hex),
                    bytes.get(i + 2).copied().and_then(hex),
                ) else {
                    return Err(PathError::BadEscape);
                };
                let v = hi << 4 | lo;
                if v == 0 {
                    return Err(PathError::Nul);
                }
                if unreserved(v) {
                    out.push(char::from(v));
                } else {
                    push_escape(&mut out, v);
                }
                i += 3;
            }
            0 => return Err(PathError::Nul),
            b'\\' => return Err(PathError::Backslash),
            _ => {
                push_byte(&mut out, b);
                i += 1;
            }
        }
    }
    if has_dot_segment(&out) {
        return Err(PathError::DotSegment);
    }
    Ok(out)
}

fn dot(segment: &str) -> bool {
    let name = segment.split(';').next().unwrap_or(segment);
    name == "." || name == ".."
}

fn has_dot_segment(normalized: &str) -> bool {
    normalized
        .split('/')
        .flat_map(|s| s.split("%2F"))
        .flat_map(|s| s.split("%5C"))
        .any(dot)
}

fn separator_len(rest: &str) -> usize {
    if rest.starts_with('/') {
        1
    } else if rest.starts_with("%2F") || rest.starts_with("%5C") {
        3
    } else {
        0
    }
}

struct Segment {
    start: usize,
    name_end: usize,
}

fn segments(path: &str) -> impl Iterator<Item = Segment> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        loop {
            let n = separator_len(&path[i..]);
            if n == 0 {
                break;
            }
            i += n;
        }
        if i >= path.len() {
            return None;
        }
        let start = i;
        while i < path.len() && separator_len(&path[i..]) == 0 {
            i += path[i..].chars().next().map_or(1, char::len_utf8);
        }
        let name_end = path[start..i].find(';').map_or(i, |p| start + p);
        Some(Segment { start, name_end })
    })
}

fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn match_view(normalized: &str) -> Cow<'_, str> {
    let mut view = String::with_capacity(normalized.len());
    for seg in segments(normalized) {
        if seg.name_end > seg.start {
            view.push('/');
            view.push_str(&normalized[seg.start..seg.name_end].to_ascii_lowercase());
        }
    }
    if view.is_empty() || normalized.ends_with('/') {
        view.push('/');
    }
    if view == normalized {
        Cow::Borrowed(normalized)
    } else {
        Cow::Owned(view)
    }
}

pub fn prefix_end(prefix: &str, normalized: &str) -> Option<usize> {
    let mut path = segments(normalized).filter(|s| s.name_end > s.start);
    let mut end = 0;
    for want in prefix.split('/').filter(|p| !p.is_empty()) {
        let seg = path.next()?;
        if !same_name(&normalized[seg.start..seg.name_end], want) {
            return None;
        }
        end = seg.name_end;
    }
    Some(end)
}

pub fn on_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

pub fn path_matches(prefix: &str, normalized: &str) -> bool {
    prefix_end(prefix, normalized).is_some()
}

pub fn route_path(text: &str) -> Result<String, String> {
    let t = text.trim();
    let wrong = || format!("path \"{text}\" must start with / and hold no ?, # or *, like /api");
    if t.contains(['?', '#', '*', ';']) {
        return Err(wrong());
    }
    let normalized = normalize_path(t).map_err(|_| wrong())?;
    if normalized.contains("%2F") || normalized.contains("%5C") {
        return Err(wrong());
    }
    let trimmed = normalized.trim_end_matches('/');
    Ok(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

pub fn route_key(text: &str) -> Result<String, String> {
    route_path(text).map(|p| p.to_ascii_lowercase())
}

#[cfg(test)]
mod tests;
