#![no_main]

use burrow::config::model::Cipher;
use burrow::noise::{Initiator, Responder};
use burrow::protocol::{ClientMsg, Connect, Hello, ServerMsg, Status, VisitRequest, decode_datagram};
use burrow::secret::Secret;
use burrow::record::RecordReader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = ServerMsg::decode(data) {
        assert_eq!(ServerMsg::decode(&m.encode()), Ok(m));
    }
    let _ = ClientMsg::decode(data);
    let _ = Connect::decode(data);
    let _ = Hello::decode(data);
    let _ = Status::decode(data);
    if let Ok(r) = VisitRequest::decode(data) {
        assert_eq!(VisitRequest::decode(&r.encode().unwrap()), Ok(r));
    }
    let _ = decode_datagram(data);

    let secret = Secret::from_bytes([9u8; 32]);
    let (init, m1) = Initiator::start(Cipher::Chachapoly, &secret, b"f");
    let resp = Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&secret), b"f", &m1).unwrap();
    let (server, m2) = resp.reply().unwrap();
    let _client = init.finish(&m2).unwrap();
    let (_, opener) = server.split();
    let mut reader = RecordReader::new(opener);
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(async {
        let mut src: &[u8] = data;
        for _ in 0..64 {
            match reader.recv(&mut src).await {
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
    });
});
