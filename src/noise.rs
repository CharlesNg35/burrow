use std::sync::Arc;

use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, StatelessTransportState};

use crate::config::check::MAX_SECRETS_PER_CLIENT;
use crate::config::model::Cipher;
use crate::secret::Secret;

pub const TAG_LEN: usize = 16;
pub const MAX_RECORD: usize = 65535;
pub const MAX_PLAINTEXT: usize = MAX_RECORD - TAG_LEN;
pub const MSG1_LEN: usize = 32 + TAG_LEN;
pub const MSG2_LEN: usize = 32 + TAG_LEN;

const CHACHA: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const AESGCM: &str = "Noise_NNpsk0_25519_AESGCM_BLAKE2s";

pub fn pattern(cipher: Cipher) -> &'static str {
    match cipher {
        Cipher::Chachapoly => CHACHA,
        Cipher::AesGcm => AESGCM,
    }
}

fn params(cipher: Cipher) -> NoiseParams {
    pattern(cipher)
        .parse()
        .expect("static noise pattern parses")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    Rejected,
    Malformed,
}

pub struct Initiator {
    state: HandshakeState,
}

impl Initiator {
    pub fn start(cipher: Cipher, secret: &Secret, prologue: &[u8]) -> (Self, [u8; MSG1_LEN]) {
        let mut state = Builder::new(params(cipher))
            .prologue(prologue)
            .and_then(|b| b.psk(0, secret.as_bytes()))
            .and_then(|b| b.build_initiator())
            .expect("valid noise parameters");
        let mut msg = [0u8; MSG1_LEN];
        let n = state
            .write_message(&[], &mut msg)
            .expect("first message fits");
        debug_assert_eq!(n, MSG1_LEN);
        (Initiator { state }, msg)
    }

    pub fn finish(mut self, msg2: &[u8; MSG2_LEN]) -> Result<Session, HandshakeError> {
        let mut scratch = [0u8; MSG2_LEN];
        self.state
            .read_message(msg2, &mut scratch)
            .map_err(|_| HandshakeError::Rejected)?;
        Session::from_handshake(self.state)
    }
}

pub struct Responder {
    state: HandshakeState,
}

impl Responder {
    pub fn accept(
        cipher: Cipher,
        candidates: &[Secret],
        prologue: &[u8],
        msg1: &[u8; MSG1_LEN],
    ) -> Result<Self, HandshakeError> {
        let mut accepted = None;
        let dummies: [Secret; MAX_SECRETS_PER_CLIENT] = std::array::from_fn(|_| Secret::generate());
        for (i, dummy) in dummies.iter().enumerate() {
            let psk = candidates.get(i).unwrap_or(dummy);
            let mut state = Builder::new(params(cipher))
                .prologue(prologue)
                .and_then(|b| b.psk(0, psk.as_bytes()))
                .and_then(|b| b.build_responder())
                .expect("valid noise parameters");
            let mut scratch = [0u8; MSG1_LEN];
            let ok = state.read_message(msg1, &mut scratch).is_ok();
            if ok && accepted.is_none() && i < candidates.len() {
                accepted = Some(state);
            }
        }
        accepted
            .map(|state| Responder { state })
            .ok_or(HandshakeError::Rejected)
    }

    pub fn reply(mut self) -> Result<(Session, [u8; MSG2_LEN]), HandshakeError> {
        let mut msg = [0u8; MSG2_LEN];
        self.state
            .write_message(&[], &mut msg)
            .map_err(|_| HandshakeError::Malformed)?;
        Ok((Session::from_handshake(self.state)?, msg))
    }
}

#[derive(Clone)]
pub struct Session {
    state: Arc<StatelessTransportState>,
}

impl Session {
    fn from_handshake(state: HandshakeState) -> Result<Self, HandshakeError> {
        let state = state
            .into_stateless_transport_mode()
            .map_err(|_| HandshakeError::Malformed)?;
        Ok(Session {
            state: Arc::new(state),
        })
    }

    pub fn split(&self) -> (Sealer, Opener) {
        (
            Sealer {
                state: self.state.clone(),
                nonce: 0,
            },
            Opener {
                state: self.state.clone(),
                nonce: 0,
            },
        )
    }
}

pub struct Sealer {
    state: Arc<StatelessTransportState>,
    nonce: u64,
}

impl Sealer {
    pub fn seal(&mut self, plain: &[u8], out: &mut [u8]) -> usize {
        let n = self
            .state
            .write_message(self.nonce, plain, out)
            .expect("plaintext and buffer sizes are checked by the caller");
        self.nonce += 1;
        n
    }
}

pub struct Opener {
    state: Arc<StatelessTransportState>,
    nonce: u64,
}

impl Opener {
    pub fn open(&mut self, cipher: &[u8], out: &mut [u8]) -> Option<usize> {
        let n = self.state.read_message(self.nonce, cipher, out).ok()?;
        self.nonce += 1;
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handshake(
        client_secret: &Secret,
        server: &[Secret],
        cp: &[u8],
        sp: &[u8],
    ) -> Result<(Session, Session), HandshakeError> {
        let (init, msg1) = Initiator::start(Cipher::Chachapoly, client_secret, cp);
        let resp = Responder::accept(Cipher::Chachapoly, server, sp, &msg1)?;
        let (srv, msg2) = resp.reply()?;
        let cli = init.finish(&msg2)?;
        Ok((cli, srv))
    }

    #[test]
    fn round_trip() {
        let s = Secret::generate();
        let (cli, srv) = handshake(&s, std::slice::from_ref(&s), b"p", b"p").unwrap();
        let (mut seal, _) = cli.split();
        let (_, mut open) = srv.split();
        let mut c = [0u8; 64];
        let mut p = [0u8; 64];
        for msg in [&b"hello"[..], b"", b"world"] {
            let n = seal.seal(msg, &mut c);
            let m = open.open(&c[..n], &mut p).unwrap();
            assert_eq!(&p[..m], msg);
        }
    }

    #[test]
    fn wrong_secret_rejected() {
        let a = Secret::generate();
        let b = Secret::generate();
        assert_eq!(
            handshake(&a, &[b], b"p", b"p").err(),
            Some(HandshakeError::Rejected)
        );
    }

    #[test]
    fn prologue_mismatch_rejected() {
        let s = Secret::generate();
        assert!(handshake(&s, std::slice::from_ref(&s), b"name=a", b"name=b").is_err());
    }

    #[test]
    fn rotation_accepts_any_listed_secret() {
        let old = Secret::generate();
        let new = Secret::generate();
        let list = [old.clone(), new.clone()];
        assert!(handshake(&old, &list, b"p", b"p").is_ok());
        assert!(handshake(&new, &list, b"p", b"p").is_ok());
        assert!(handshake(&Secret::generate(), &list, b"p", b"p").is_err());
    }

    #[test]
    fn tampered_messages_rejected() {
        let s = Secret::generate();
        let (init, mut msg1) = Initiator::start(Cipher::Chachapoly, &s, b"p");
        msg1[40] ^= 1;
        assert!(
            Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"p", &msg1).is_err()
        );

        let (init2, msg1) = Initiator::start(Cipher::Chachapoly, &s, b"p");
        let resp =
            Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"p", &msg1).unwrap();
        let (_, mut msg2) = resp.reply().unwrap();
        msg2[3] ^= 1;
        assert!(init2.finish(&msg2).is_err());
        drop(init);
    }

    #[test]
    fn replayed_first_message_gives_unrelated_keys() {
        let s = Secret::generate();
        let (init, msg1) = Initiator::start(Cipher::Chachapoly, &s, b"p");
        let r1 =
            Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"p", &msg1).unwrap();
        let (srv1, msg2) = r1.reply().unwrap();
        let cli = init.finish(&msg2).unwrap();

        let r2 =
            Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"p", &msg1).unwrap();
        let (srv2, _) = r2.reply().unwrap();

        let (mut seal, _) = cli.split();
        let mut c = [0u8; 64];
        let n = seal.seal(b"hello", &mut c);
        let mut p = [0u8; 64];
        assert!(srv1.split().1.open(&c[..n], &mut p).is_some());
        assert!(srv2.split().1.open(&c[..n], &mut p).is_none());
    }

    #[test]
    fn out_of_order_records_rejected() {
        let s = Secret::generate();
        let (cli, srv) = handshake(&s, std::slice::from_ref(&s), b"p", b"p").unwrap();
        let (mut seal, _) = cli.split();
        let (_, mut open) = srv.split();
        let mut c1 = [0u8; 64];
        let mut c2 = [0u8; 64];
        let n1 = seal.seal(b"one", &mut c1);
        let n2 = seal.seal(b"two", &mut c2);
        let mut p = [0u8; 64];
        assert!(open.open(&c2[..n2], &mut p).is_none());
        assert!(open.open(&c1[..n1], &mut p).is_some());
    }

    #[test]
    fn aes_and_chacha_do_not_interoperate() {
        let s = Secret::generate();
        let (_, msg1) = Initiator::start(Cipher::AesGcm, &s, b"p");
        assert!(
            Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"p", &msg1).is_err()
        );
        assert!(Responder::accept(Cipher::AesGcm, std::slice::from_ref(&s), b"p", &msg1).is_ok());
    }
}
