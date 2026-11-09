use std::io::Write;

use burrow_syntax::secret_fmt::hide_url_passwords;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub fn json(v: &mut Value) {
    match v {
        Value::String(s) if s.contains("://") => *s = hide_url_passwords(s),
        Value::Array(items) => items.iter_mut().for_each(json),
        Value::Object(m) => m.values_mut().for_each(json),
        _ => {}
    }
}

pub fn data<T: Serialize + DeserializeOwned>(t: T) -> T {
    let Ok(mut v) = serde_json::to_value(&t) else {
        return t;
    };
    json(&mut v);
    serde_json::from_value(v).unwrap_or(t)
}

pub struct Logs<W>(pub W);

impl<W: Write> Write for Logs<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match std::str::from_utf8(buf) {
            Ok(text) if text.contains("://") => {
                self.0.write_all(hide_url_passwords(text).as_bytes())?
            }
            _ => self.0.write_all(buf)?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_and_log_lines_lose_the_password_of_every_url() {
        let mut v = serde_json::json!({
            "a": ["https://acme:pw-in-a-list@ca.internal/dir"],
            "b": {"c": "the test service https://acme:pw-in-a-map@ca.internal could not be reached"},
            "n": 7,
        });
        json(&mut v);
        let sent = v.to_string();
        assert!(!sent.contains("pw-in-a"), "{sent}");
        assert!(sent.contains("https://«hidden»@ca.internal/dir"), "{sent}");
        let mut out = Logs(Vec::new());
        out.write_all(b"ACME account 7 created at https://acme:pw-in-a-log@ca.internal/dir\n")
            .unwrap();
        let line = String::from_utf8(out.0).unwrap();
        assert_eq!(
            line,
            "ACME account 7 created at https://«hidden»@ca.internal/dir\n"
        );
    }
}
