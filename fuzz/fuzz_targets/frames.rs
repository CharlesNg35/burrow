#![no_main]

use burrow::wire::noise::{Initiator, Responder};
use burrow::wire::protocol::{
    ClientMsg, Connect, Grants, Hello, ServerMsg, Status, VisitRequest, decode_datagram,
};
use burrow::wire::record::RecordReader;
use burrow::wire::secret::Secret;
use burrow_config::Cipher;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = ServerMsg::decode(data) {
        assert_eq!(ServerMsg::decode(&m.encode()), Ok(m));
    }
    if let Ok(m) = ClientMsg::decode(data) {
        assert_eq!(ClientMsg::decode(&m.encode()), Ok(m));
    }
    if let Ok(c) = Connect::decode(data) {
        assert_eq!(Connect::decode(&c.encode()), Ok(c));
    }
    if let Ok(h) = Hello::decode(data) {
        assert_eq!(Hello::decode(&h.encode()), Ok(h));
    }
    let _ = Status::decode(data);
    if let Ok(r) = VisitRequest::decode(data) {
        assert_eq!(VisitRequest::decode(&r.encode().unwrap()), Ok(r));
    }
    let _ = decode_datagram(data);
    if let Ok(g) = Grants::decode(data) {
        if let Ok(b) = g.encode_v2() {
            assert_eq!(Grants::decode(&b).unwrap().services, g.services);
        }
        if let Ok(b) = g.encode() {
            let tcp = Grants::decode(&b).unwrap();
            assert!(tcp.services.iter().all(|s| g.services.contains(s)));
        }
    }

    let secret = Secret::from_bytes([9u8; 32]);
    let (init, m1) = Initiator::start(Cipher::Chachapoly, &secret, b"f");
    let resp =
        Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&secret), b"f", &m1).unwrap();
    let (server, m2) = resp.reply().unwrap();
    let _client = init.finish(&m2).unwrap();
    let (_, opener) = server.split();
    let mut reader = RecordReader::new(opener);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
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
