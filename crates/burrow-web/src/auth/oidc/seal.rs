use std::path::Path;

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tracing::warn;
use zeroize::Zeroizing;

pub const KEY_FILE: &str = "login-session.key";
const KEY_LEN: usize = 32;

pub(crate) struct Sealer(LessSafeKey);

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    aws_lc_rs::rand::fill(&mut b).expect("the system random source");
    b
}

fn read_key(path: &Path) -> Option<Zeroizing<Vec<u8>>> {
    let bytes = Zeroizing::new(std::fs::read(path).ok()?);
    (bytes.len() == KEY_LEN).then_some(bytes)
}

fn stored_key(dir: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    let path = dir.join(KEY_FILE);
    if let Some(k) = read_key(&path) {
        return Ok(k);
    }
    let fresh = Zeroizing::new(random::<KEY_LEN>().to_vec());
    crate::tls::acme::storage::write_private(&path, &fresh)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(fresh)
}

impl Sealer {
    pub(crate) fn from_bytes(key: &[u8]) -> Sealer {
        let key = UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key");
        Sealer(LessSafeKey::new(key))
    }

    pub(crate) fn load(dir: Option<&Path>) -> Sealer {
        let key = match dir.map(stored_key) {
            Some(Ok(k)) => k,
            Some(Err(e)) => {
                warn!("web: login sessions: {e}; logins last until burrow restarts");
                Zeroizing::new(random::<KEY_LEN>().to_vec())
            }
            None => Zeroizing::new(random::<KEY_LEN>().to_vec()),
        };
        Sealer::from_bytes(&key)
    }

    pub(crate) fn seal(&self, aad: &str, data: &[u8]) -> String {
        let nonce = random::<NONCE_LEN>();
        let mut out = data.to_vec();
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut out,
            )
            .expect("sealing a short value");
        let mut all = nonce.to_vec();
        all.extend_from_slice(&out);
        URL_SAFE_NO_PAD.encode(all)
    }

    pub(crate) fn open(&self, aad: &str, text: &str) -> Option<Vec<u8>> {
        let mut all = URL_SAFE_NO_PAD.decode(text).ok()?;
        if all.len() < NONCE_LEN {
            return None;
        }
        let mut sealed = all.split_off(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(&all).ok()?;
        let plain = self
            .0
            .open_in_place(nonce, Aad::from(aad.as_bytes()), &mut sealed)
            .ok()?;
        Some(plain.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_value_opens_only_with_its_key_and_its_purpose() {
        let s = Sealer::from_bytes(&[7; KEY_LEN]);
        let text = s.seal("session|a.example.com", b"alice");
        assert_eq!(
            s.open("session|a.example.com", &text).as_deref(),
            Some(&b"alice"[..])
        );
        assert_eq!(s.open("session|b.example.com", &text), None);
        assert_eq!(
            Sealer::from_bytes(&[8; KEY_LEN]).open("session|a.example.com", &text),
            None
        );
        let mut bad = text.into_bytes();
        let last = bad.len() - 2;
        bad[last] = if bad[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            s.open("session|a.example.com", &String::from_utf8(bad).unwrap()),
            None
        );
        assert_eq!(s.open("x", "short"), None);
        assert_ne!(
            s.seal("x", b"same"),
            s.seal("x", b"same"),
            "a fresh nonce each time"
        );
    }

    #[test]
    fn the_key_is_kept_privately_in_the_state_folder_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = Sealer::load(Some(dir.path()));
        let text = a.seal("x", b"kept");
        let b = Sealer::load(Some(dir.path()));
        assert_eq!(b.open("x", &text).as_deref(), Some(&b"kept"[..]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(dir.path().join(KEY_FILE), b"short").unwrap();
        let c = Sealer::load(Some(dir.path()));
        assert_eq!(c.open("x", &text), None, "a damaged key file is replaced");
    }
}
