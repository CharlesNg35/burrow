use std::collections::BTreeMap;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::kit::*;

const WINDOW_UPDATE: u8 = 8;
const HEADER_TABLE_SIZE: u16 = 1;
const ENABLE_PUSH: u16 = 2;
const MAX_CONCURRENT_STREAMS: u16 = 3;
const INITIAL_WINDOW_SIZE: u16 = 4;
const MAX_FRAME_SIZE: u16 = 5;
const MAX_HEADER_LIST_SIZE: u16 = 6;
const RFC_DEFAULTS: [(u16, u32); 3] = [
    (HEADER_TABLE_SIZE, 4096),
    (INITIAL_WINDOW_SIZE, 65_535),
    (MAX_FRAME_SIZE, 16_384),
];

fn settings_of(payload: &[u8]) -> BTreeMap<u16, u32> {
    let mut got: BTreeMap<u16, u32> = RFC_DEFAULTS.into_iter().collect();
    for p in payload.as_chunks::<6>().0 {
        got.insert(
            u16::from_be_bytes([p[0], p[1]]),
            u32::from_be_bytes([p[2], p[3], p[4], p[5]]),
        );
    }
    got
}

#[tokio::test]
async fn the_server_announces_the_decided_settings_and_connection_window() {
    let a = app().await;
    let p = proxy(&a, |_| {}).await;
    let t = tls_with(p.https_addr().unwrap(), HOST, &[H2])
        .await
        .unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H2));
    let (mut rd, mut wr) = tokio::io::split(t);
    let mut hello = PREFACE.to_vec();
    hello.extend(frame(SETTINGS, 0, 0, &[]));
    wr.write_all(&hello).await.unwrap();
    let mut settings = None;
    let mut conn_window = None;
    while settings.is_none() || conn_window.is_none() {
        let f = read_frame(&mut rd, Duration::from_secs(5))
            .await
            .expect("the server's first frames");
        match (f.ty, f.stream) {
            (SETTINGS, 0) if f.flags & ACK == 0 => {
                settings = Some(settings_of(&f.payload));
                wr.write_all(&frame(SETTINGS, ACK, 0, &[])).await.unwrap();
            }
            (WINDOW_UPDATE, 0) => {
                let n = u32::from_be_bytes(f.payload[..4].try_into().unwrap()) & 0x7fff_ffff;
                conn_window = Some(65_535 + n);
            }
            _ => {}
        }
    }
    let s = settings.unwrap();
    let want = [
        (HEADER_TABLE_SIZE, 4096),
        (MAX_CONCURRENT_STREAMS, 100),
        (INITIAL_WINDOW_SIZE, 256 * 1024),
        (MAX_FRAME_SIZE, 16 * 1024),
        (MAX_HEADER_LIST_SIZE, 32 * 1024),
    ];
    for (id, value) in want {
        assert_eq!(s.get(&id), Some(&value), "setting {id} in {s:?}");
    }
    assert_ne!(s.get(&ENABLE_PUSH), Some(&1), "a server never enables push");
    assert_eq!(conn_window, Some(1024 * 1024), "the connection window");
}
