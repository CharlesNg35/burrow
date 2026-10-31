use super::*;

#[test]
fn a_datagram_with_its_peer_id_fills_one_record_at_most() {
    assert_eq!(MAX_DATAGRAM, 65515);
    let mut rec = Vec::new();
    encode_datagram(7, &vec![1u8; MAX_DATAGRAM], &mut rec);
    assert_eq!(rec.len(), crate::wire::noise::MAX_PLAINTEXT);
    let (id, payload) = decode_datagram(&rec).unwrap();
    assert_eq!((id, payload.len()), (7, MAX_DATAGRAM));
}

#[test]
fn a_grants_answer_rejects_what_cannot_be_true() {
    let one = |port: u16, ports: u16| Grants {
        services: vec![GrantedService {
            name: "db".into(),
            protocol: Protocol::Tcp,
            port,
            ports,
        }],
        clients: vec![],
    };
    assert!(Grants::decode(&one(5432, 1).encode().unwrap()).is_ok());
    assert!(Grants::decode(&one(65535, 1).encode().unwrap()).is_ok());
    assert!(Grants::decode(&one(5432, 0).encode().unwrap()).is_err());
    assert!(Grants::decode(&one(65000, 1000).encode().unwrap()).is_err());
    assert!(Grants::decode(&one(1, 1001).encode().unwrap()).is_err());
    assert!(Grants::decode(&one(64536, 1000).encode().unwrap()).is_ok());
    let evil = ClientMsg::Allow(vec!["unix:/x\n  evil:".into()]).encode();
    assert!(ClientMsg::decode(&evil).is_err());
}

#[test]
fn a_large_answer_drops_allow_lists_first() {
    let rule = "10.0.0.0/8:1-65535".repeat(10);
    let clients: Vec<GrantedClient> = (0..60)
        .map(|i| GrantedClient {
            name: format!("client-{i}"),
            reach: vec![],
            allow: Some(vec![rule.clone(); MAX_ALLOW_RULES]),
        })
        .collect();
    let big = Grants {
        services: vec![],
        clients,
    };
    let sent = Grants::decode(&big.encode().unwrap()).unwrap();
    assert_eq!(sent.clients.len(), 60);
    assert!(sent.clients.iter().all(|c| c.allow.is_none()));
    let huge = Grants {
        services: (0..2000)
            .map(|i| GrantedService {
                name: format!("service-with-a-rather-long-name-{i:04}"),
                protocol: Protocol::Tcp,
                port: 1,
                ports: 1,
            })
            .collect(),
        clients: vec![],
    };
    assert!(huge.encode().is_err());
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn grants_travel_with_known_and_unknown_allow_lists() {
    let grants = Grants {
        services: vec![GrantedService {
            name: "db".into(),
            protocol: Protocol::Tcp,
            port: 5432,
            ports: 1,
        }],
        clients: vec![
            GrantedClient {
                name: "home".into(),
                reach: vec![],
                allow: Some(vec!["127.0.0.0/8".into()]),
            },
            GrantedClient {
                name: "office".into(),
                reach: vec!["10.0.0.0/8".into()],
                allow: None,
            },
        ],
    };
    let bytes = grants.encode().unwrap();
    assert_eq!(
        bytes,
        unhex(
            "22000102646215380001000204686f6d650001010b3132372e302e302e302f38066f6666696365010a31302e302e302e302f3800"
        )
    );
    assert_eq!(Grants::decode(&bytes).unwrap(), grants);
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Grants::decode(&trailing).is_err());
    let bad = Grants {
        services: vec![GrantedService {
            name: "no spaces".into(),
            protocol: Protocol::Tcp,
            port: 1,
            ports: 1,
        }],
        clients: vec![],
    };
    assert!(Grants::decode(&bad.encode().unwrap()).is_err());
}

#[test]
fn an_allow_list_is_capped_on_the_wire() {
    let many: Vec<String> = (0..100).map(|i| format!("10.0.{i}.0/24")).collect();
    let sent = ClientMsg::Allow(many.clone()).encode();
    let ClientMsg::Allow(got) = ClientMsg::decode(&sent).unwrap() else {
        panic!("an allow message")
    };
    assert_eq!(got, many[..MAX_ALLOW_RULES].to_vec());
    let mut too_many = vec![0x82, (MAX_ALLOW_RULES + 1) as u8];
    for _ in 0..=MAX_ALLOW_RULES {
        too_many.extend_from_slice(&[1, b'x']);
    }
    assert!(ClientMsg::decode(&too_many).is_err());
    assert_eq!(
        ClientMsg::decode(&ClientMsg::Pong.encode()).unwrap(),
        ClientMsg::Pong
    );
}

#[test]
fn the_wire_format_stays_as_documented() {
    let services = ServerMsg::Services(vec![
        ServiceInfo {
            name: "web".into(),
            protocol: Protocol::Tcp,
            bind_port: 443,
            target: "127.0.0.1:8443".into(),
            ports: 1,
        },
        ServiceInfo {
            name: "game".into(),
            protocol: Protocol::Udp,
            bind_port: 30000,
            target: "192.168.1.50:30000".into(),
            ports: 11,
        },
        ServiceInfo {
            name: "db".into(),
            protocol: Protocol::Tcp,
            bind_port: 0,
            target: "unix:/run/my-db.sock".into(),
            ports: 1,
        },
    ]);
    let server = [
        (
            ServerMsg::Welcome {
                token: [9; 16],
                heartbeat_secs: 10,
            },
            "0109090909090909090909090909090909000a",
        ),
        (
            services,
            "020003037765620001bb0e3132372e302e302e313a3834343300010467616d65017530123139322e3136382e312e35303a3330303030000b02646200000014756e69783a2f72756e2f6d792d64622e736f636b0001",
        ),
        (ServerMsg::Ping, "03"),
        (ServerMsg::OpenData { count: 7 }, "040007"),
        (
            ServerMsg::Bye {
                reason: ByeReason::Replaced,
            },
            "0502",
        ),
    ];
    for (msg, wire) in server {
        assert_eq!(msg.encode(), unhex(wire), "{msg:?}");
        assert_eq!(ServerMsg::decode(&unhex(wire)).unwrap(), msg);
    }
    assert_eq!(ClientMsg::decode(&unhex("81")).unwrap(), ClientMsg::Pong);
    let preambles = [
        (
            Preamble {
                kind: ChannelKind::Control,
                cipher: Cipher::Chachapoly,
                plaintext: false,
                name: "home".into(),
            },
            "42525731010004686f6d65",
        ),
        (
            Preamble {
                kind: ChannelKind::Data,
                cipher: Cipher::AesGcm,
                plaintext: true,
                name: "home".into(),
            },
            "42525731020304686f6d65",
        ),
    ];
    for (p, wire) in preambles {
        let b = unhex(wire);
        assert_eq!(p.encode(), b);
        let head: [u8; PREAMBLE_FIXED] = b[..PREAMBLE_FIXED].try_into().unwrap();
        let (kind, flags, _) = Preamble::decode_fixed(&head).unwrap();
        assert_eq!(
            Preamble::from_parts(kind, flags, &b[PREAMBLE_FIXED..]).unwrap(),
            p
        );
    }
    for (hello, wire) in [
        (
            Hello::Control {
                version: 1,
                instance: [9; 16],
            },
            "010109090909090909090909090909090909",
        ),
        (
            Hello::Data {
                version: 1,
                token: [7; 16],
            },
            "020107070707070707070707070707070707",
        ),
    ] {
        assert_eq!(hello.encode(), unhex(wire));
        assert_eq!(Hello::decode(&unhex(wire)).unwrap(), hello);
    }
    let connect = Connect {
        protocol: Protocol::Udp,
        service: "dns".into(),
        target: "192.168.1.1:53".into(),
        only: None,
    };
    let wire = unhex("100103646e730e3139322e3136382e312e313a353300");
    assert_eq!(connect.encode(), wire);
    assert_eq!(Connect::decode(&wire).unwrap(), connect);
    for (status, byte) in [
        (Status::Ok, 0),
        (Status::Denied, 1),
        (Status::Unreachable, 2),
        (Status::Unresolved, 3),
        (Status::Invalid, 4),
    ] {
        assert_eq!(Status::decode(&[byte]).unwrap(), status);
    }
    let mut out = Vec::new();
    encode_datagram(5, b"hi", &mut out);
    assert_eq!(out, unhex("000000056869"));
}

#[test]
fn fields_too_long_for_the_wire_are_an_error() {
    let too_long = VisitRequest {
        check: false,
        protocol: Protocol::Tcp,
        to: VisitTarget::Client {
            client: "home".into(),
            target: format!("{}:8080", "a".repeat(252)),
        },
    };
    assert_eq!(too_long.encode(), Err(DecodeError::TooLong));
}

#[test]
fn ports_of_a_range_are_shown_from_one() {
    assert_eq!(VisitTarget::service("game").to_string(), "service \"game\"");
    let second = VisitTarget::Service {
        name: "game".into(),
        index: 1,
    };
    assert_eq!(second.to_string(), "service \"game\" port 2");
}

#[test]
fn preamble_round_trip() {
    let p = Preamble {
        kind: ChannelKind::Data,
        cipher: Cipher::AesGcm,
        plaintext: true,
        name: "home".into(),
    };
    let b = p.encode();
    let head: [u8; PREAMBLE_FIXED] = b[..PREAMBLE_FIXED].try_into().unwrap();
    let (kind, flags, len) = Preamble::decode_fixed(&head).unwrap();
    assert_eq!(len, 4);
    assert_eq!(
        Preamble::from_parts(kind, flags, &b[PREAMBLE_FIXED..]).unwrap(),
        p
    );
}

#[test]
fn preamble_rejects_garbage() {
    assert_eq!(
        Preamble::decode_fixed(b"GET / H"),
        Err(DecodeError::BadMagic)
    );
    assert_eq!(
        Preamble::decode_fixed(b"BRW1\x04\x00\x01"),
        Err(DecodeError::BadKind)
    );
    assert_eq!(
        Preamble::decode_fixed(b"BRW1\x01\x04\x01"),
        Err(DecodeError::BadFlags)
    );
    assert_eq!(
        Preamble::decode_fixed(b"BRW1\x01\x00\x00"),
        Err(DecodeError::BadName)
    );
    assert_eq!(
        Preamble::decode_fixed(b"BRW1\x01\x00\x41"),
        Err(DecodeError::BadName)
    );
    assert_eq!(
        Preamble::from_parts(ChannelKind::Control, 0, b"a b"),
        Err(DecodeError::BadName)
    );
}

#[test]
fn messages_round_trip() {
    let msgs = [
        ServerMsg::Welcome {
            token: [9; 16],
            heartbeat_secs: 10,
        },
        ServerMsg::Services(vec![ServiceInfo {
            name: "web".into(),
            protocol: Protocol::Udp,
            bind_port: 4000,
            target: "127.0.0.1:3000".into(),
            ports: 11,
        }]),
        ServerMsg::Ping,
        ServerMsg::OpenData { count: 7 },
        ServerMsg::Bye {
            reason: ByeReason::Replaced,
        },
    ];
    for m in msgs {
        assert_eq!(ServerMsg::decode(&m.encode()).unwrap(), m);
    }
    assert_eq!(
        ClientMsg::decode(&ClientMsg::Pong.encode()).unwrap(),
        ClientMsg::Pong
    );
    let c = Connect {
        protocol: Protocol::Tcp,
        service: "web".into(),
        target: "db.lan:5432".into(),
        only: Some(vec!["192.168.150.0/25".into(), "unix:/run/app.sock".into()]),
    };
    assert_eq!(Connect::decode(&c.encode()).unwrap(), c);
    let none = Connect {
        only: None,
        ..c.clone()
    };
    assert_eq!(Connect::decode(&none.encode()).unwrap(), none);
    let empty = Connect {
        only: Some(vec![]),
        ..c.clone()
    };
    assert_eq!(Connect::decode(&empty.encode()).unwrap().only, Some(vec![]));
    let h = Hello::Data {
        version: 1,
        token: [3; 16],
    };
    assert_eq!(Hello::decode(&h.encode()).unwrap(), h);
    let v = Hello::Visit { version: 1 };
    assert_eq!(Hello::decode(&v.encode()).unwrap(), v);
}

#[test]
fn visit_requests_round_trip() {
    for r in [
        VisitRequest {
            check: false,
            protocol: Protocol::Tcp,
            to: VisitTarget::service("db"),
        },
        VisitRequest {
            check: true,
            protocol: Protocol::Tcp,
            to: VisitTarget::Client {
                client: "home".into(),
                target: "[::1]:22".into(),
            },
        },
    ] {
        let b = r.encode().unwrap();
        assert_eq!(VisitRequest::decode(&b).unwrap(), r);
        for i in 0..b.len() {
            assert!(VisitRequest::decode(&b[..i]).is_err());
        }
        let mut t = b.clone();
        t.push(0);
        assert_eq!(VisitRequest::decode(&t), Err(DecodeError::Trailing));
    }
    assert!(VisitRequest::decode(&[0x20, 2, 1, 2, b'd', b'b']).is_err());
    assert_eq!(
        VisitRequest::decode(&[0x20, 4, 1, 2, b'd', b'b', 0, 0]),
        Err(DecodeError::UnknownType(4))
    );
    assert_eq!(
        VisitRequest::decode(&[0x20, 0x80, 1, 2, b'd', b'b', 0, 0]),
        Err(DecodeError::UnknownType(0x80))
    );
    assert!(VisitRequest::decode(&[0x20, 0, 3, 2, b'd', b'b']).is_err());
    assert_eq!(
        VisitRequest::decode(&[0x20, 0, 1, 3, b'a', b' ', b'b']),
        Err(DecodeError::BadName)
    );
    assert_eq!(
        VisitRequest::decode(&[0x10, 0, 1, 2, b'd', b'b']),
        Err(DecodeError::UnknownType(0x10))
    );
}

#[test]
fn a_udp_visit_request_sets_flag_bit_1_and_a_tcp_one_keeps_the_v1_bytes() {
    let tcp = VisitRequest {
        check: false,
        protocol: Protocol::Tcp,
        to: VisitTarget::service("db"),
    };
    assert_eq!(tcp.encode().unwrap(), vec![0x20, 0, 1, 2, b'd', b'b', 0, 0]);
    let udp = VisitRequest {
        protocol: Protocol::Udp,
        ..tcp.clone()
    };
    assert_eq!(udp.encode().unwrap(), vec![0x20, 2, 1, 2, b'd', b'b', 0, 0]);
    let checked = VisitRequest {
        check: true,
        protocol: Protocol::Udp,
        to: VisitTarget::Client {
            client: "home".into(),
            target: "10.0.0.1:53".into(),
        },
    };
    let b = checked.encode().unwrap();
    assert_eq!(&b[..3], &[0x20, 3, 2]);
    for r in [udp, checked] {
        let b = r.encode().unwrap();
        assert_eq!(VisitRequest::decode(&b).unwrap(), r);
        for i in 0..b.len() {
            assert!(VisitRequest::decode(&b[..i]).is_err());
        }
    }
}

#[test]
fn grants_v2_carry_each_service_protocol_and_v1_lists_tcp_only() {
    let grants = Grants {
        services: vec![
            GrantedService {
                name: "db".into(),
                protocol: Protocol::Tcp,
                port: 5432,
                ports: 1,
            },
            GrantedService {
                name: "dns".into(),
                protocol: Protocol::Udp,
                port: 53,
                ports: 2,
            },
        ],
        clients: vec![GrantedClient {
            name: "home".into(),
            reach: vec![],
            allow: None,
        }],
    };
    assert_eq!(GRANTS_REQUEST_V2, [0x21, 0x01]);
    let v2 = grants.encode_v2().unwrap();
    assert_eq!(
        v2,
        unhex("250002026462001538000103646e730100350002000104686f6d650000")
    );
    assert_eq!(Grants::decode(&v2).unwrap(), grants);
    let v1 = grants.encode().unwrap();
    assert_eq!(v1[0], 0x22);
    let old = Grants::decode(&v1).unwrap();
    assert_eq!(old.services, grants.services[..1].to_vec());
    assert_eq!(old.clients, grants.clients);
    let mut bad = v2.clone();
    bad[6] = 2;
    assert_eq!(Grants::decode(&bad), Err(DecodeError::UnknownType(2)));
    for i in 0..v2.len() {
        assert!(Grants::decode(&v2[..i]).is_err());
    }
    let mut trailing = v2;
    trailing.push(0);
    assert!(Grants::decode(&trailing).is_err());
}

#[test]
fn statuses_decode() {
    for s in [
        Status::Ok,
        Status::Denied,
        Status::Unreachable,
        Status::Unresolved,
        Status::Invalid,
        Status::Forbidden,
        Status::Offline,
        Status::Busy,
    ] {
        assert_eq!(Status::decode(&[s as u8]), Ok(s));
    }
    assert_eq!(Status::decode(&[8]), Err(DecodeError::UnknownType(8)));
}

#[test]
fn decode_rejects_truncated_and_trailing() {
    let w = ServerMsg::Welcome {
        token: [1; 16],
        heartbeat_secs: 5,
    }
    .encode();
    for i in 0..w.len() {
        assert!(ServerMsg::decode(&w[..i]).is_err());
    }
    let mut t = w.clone();
    t.push(0);
    assert_eq!(ServerMsg::decode(&t), Err(DecodeError::Trailing));
    assert!(ServerMsg::decode(&[2, 0xff, 0xff]).is_err());
    assert_eq!(
        decode_datagram(&[0, 0, 0, 7, 1, 2]).unwrap(),
        (7, &[1u8, 2][..])
    );
    assert!(decode_datagram(&[0, 0]).is_err());
}

#[test]
fn a_peer_times_out_after_the_missed_heartbeats_the_docs_name() {
    let heartbeat = std::time::Duration::from_secs(10);
    assert_eq!(peer_timeout(heartbeat), std::time::Duration::from_secs(30));
    let docs = include_str!("../../../docs/configuration.md");
    assert!(
        docs.contains(&format!(
            "a peer times out after {MISSED_HEARTBEATS} missed pings"
        )),
        "docs/configuration.md must name {MISSED_HEARTBEATS} missed pings"
    );
}
