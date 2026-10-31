#![cfg(test)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::tsig::TsigError;
use hickory_proto::rr::rdata::{A, CNAME, NS, SOA, TXT};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType, TSigResponseContext, TSigner};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

use super::wire::name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Refuse,
    Silent,
    Forge,
    Skewed,
}

struct State {
    records: Vec<(Record, Instant)>,
    authoritative: bool,
    mode: Mode,
    key: Option<TSigner>,
    asked: Vec<(String, RecordType)>,
}

pub struct Fake {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

pub fn txt(owner: &str, value: &str) -> Record {
    Record::from_rdata(
        name(owner).unwrap(),
        60,
        RData::TXT(TXT::new(vec![value.into()])),
    )
}

pub fn cname(owner: &str, target: &str) -> Record {
    Record::from_rdata(
        name(owner).unwrap(),
        60,
        RData::CNAME(CNAME(name(target).unwrap())),
    )
}

pub fn ns(zone: &str, host: &str) -> Record {
    Record::from_rdata(name(zone).unwrap(), 60, RData::NS(NS(name(host).unwrap())))
}

pub fn a(owner: &str, ip: &str) -> Record {
    Record::from_rdata(name(owner).unwrap(), 60, RData::A(A(ip.parse().unwrap())))
}

pub fn soa(zone: &str) -> Record {
    let n = name(zone).unwrap();
    let data = SOA::new(n.clone(), n.clone(), 1, 3600, 600, 86400, 60);
    Record::from_rdata(n, 60, RData::SOA(data))
}

impl Fake {
    pub async fn start(authoritative: bool) -> Fake {
        let (udp, tcp) = loop {
            let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            if let Ok(tcp) = TcpListener::bind(udp.local_addr().unwrap()).await {
                break (udp, tcp);
            }
        };
        let addr = udp.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            records: Vec::new(),
            authoritative,
            mode: Mode::Normal,
            key: None,
            asked: Vec::new(),
        }));
        let tasks = vec![
            tokio::spawn(serve_udp(udp, state.clone())),
            tokio::spawn(serve_tcp(tcp, state.clone())),
        ];
        Fake { addr, state, tasks }
    }

    pub fn add(&self, r: Record) {
        self.add_after(r, Duration::ZERO);
    }

    pub fn add_after(&self, r: Record, delay: Duration) {
        let at = Instant::now() + delay;
        self.state.lock().unwrap().records.push((r, at));
    }

    pub fn remove(&self, owner: &str, value: &str) {
        let r = txt(owner, value);
        self.state
            .lock()
            .unwrap()
            .records
            .retain(|(x, _)| !(x.name == r.name && x.data == r.data));
    }

    pub fn mode(&self, m: Mode) {
        self.state.lock().unwrap().mode = m;
    }

    pub fn key(&self, k: TSigner) {
        self.state.lock().unwrap().key = Some(k);
    }

    pub fn asked(&self) -> Vec<(String, RecordType)> {
        self.state.lock().unwrap().asked.clone()
    }

    pub fn values(&self, owner: &str) -> Vec<String> {
        let n = name(owner).unwrap();
        let s = self.state.lock().unwrap();
        s.records
            .iter()
            .filter(|(r, _)| r.name == n)
            .filter_map(|(r, _)| match &r.data {
                RData::TXT(t) => Some(
                    t.txt_data
                        .iter()
                        .map(|d| String::from_utf8_lossy(d).into_owned())
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    }
}

async fn serve_udp(sock: UdpSocket, state: Arc<Mutex<State>>) {
    let mut buf = vec![0u8; 65535];
    while let Ok((n, from)) = sock.recv_from(&mut buf).await {
        if let Some(out) = respond(&buf[..n], &state) {
            let _ = sock.send_to(&out, from).await;
        }
    }
}

async fn serve_tcp(l: TcpListener, state: Arc<Mutex<State>>) {
    while let Ok((mut s, _)) = l.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            let mut head = [0u8; 2];
            if s.read_exact(&mut head).await.is_err() {
                return;
            }
            let mut body = vec![0u8; usize::from(u16::from_be_bytes(head))];
            if s.read_exact(&mut body).await.is_err() {
                return;
            }
            if let Some(out) = respond(&body, &state) {
                let mut framed = u16::try_from(out.len()).unwrap().to_be_bytes().to_vec();
                framed.extend_from_slice(&out);
                let _ = s.write_all(&framed).await;
            }
        });
    }
}

fn respond(raw: &[u8], state: &Mutex<State>) -> Option<Vec<u8>> {
    let req = Message::from_vec(raw).ok()?;
    let mut s = state.lock().unwrap();
    let q = req.queries.first()?.clone();
    s.asked.push((super::wire::shown(q.name()), q.query_type()));
    let mut out = Message::new(req.metadata.id, MessageType::Response, req.metadata.op_code);
    out.metadata.recursion_desired = req.metadata.recursion_desired;
    out.metadata.authoritative = s.authoritative;
    out.add_query(q.clone());
    match s.mode {
        Mode::Silent => return None,
        Mode::Refuse => {
            out.metadata.response_code = ResponseCode::Refused;
            return out.to_vec().ok();
        }
        Mode::Forge if req.metadata.op_code == OpCode::Update => return out.to_vec().ok(),
        _ => {}
    }
    if req.metadata.op_code == OpCode::Update {
        return update(&req, raw, &mut s, out);
    }
    let now = Instant::now();
    let here: Vec<&Record> = s
        .records
        .iter()
        .filter(|(r, at)| *at <= now && matches(&r.name, q.name()))
        .map(|(r, _)| r)
        .collect();
    let kind = q.query_type();
    for r in &here {
        let t = r.record_type();
        if t == kind || (t == RecordType::CNAME && kind != RecordType::CNAME) {
            let mut synthesized = (*r).clone();
            synthesized.name = q.name().clone();
            out.add_answer(synthesized);
        }
    }
    if here.is_empty() && !s.records.iter().any(|(r, _)| q.name().zone_of(&r.name)) {
        out.metadata.response_code = ResponseCode::NXDomain;
    }
    out.to_vec().ok()
}

fn update(req: &Message, raw: &[u8], s: &mut State, mut out: Message) -> Option<Vec<u8>> {
    let key = s.key.clone()?;
    let now = crate::tls::now_secs();
    let Some(sig) = req.signature() else {
        out.metadata.response_code = ResponseCode::Refused;
        return out.to_vec().ok();
    };
    let known = sig.name == *key.signer_name() && sig.data.algorithm == *key.algorithm();
    let good = known && key.verify_message_byte(raw, None, true).is_ok();
    let ctx = if !known {
        TSigResponseContext::unknown_key(req.metadata.id, now, sig.name.clone())
    } else if !good {
        TSigResponseContext::bad_signature(req.metadata.id, now, key.clone())
    } else if s.mode == Mode::Skewed {
        out.metadata.response_code = ResponseCode::NotAuth;
        let late = Some(TsigError::BadTime);
        TSigResponseContext::new(
            req.metadata.id,
            now + 3600,
            key.clone(),
            sig.data.mac.clone(),
            late,
        )
    } else {
        out.metadata.response_code = apply(req, s);
        TSigResponseContext::new(
            req.metadata.id,
            now,
            key.clone(),
            sig.data.mac.clone(),
            None,
        )
    };
    if !good {
        out.metadata.response_code = ResponseCode::NotAuth;
    }
    let body = out.to_vec().ok()?;
    let signed = ctx.sign(&body).ok()?;
    out.set_signature(signed);
    out.to_vec().ok()
}

fn apply(req: &Message, s: &mut State) -> ResponseCode {
    let zone = req.queries[0].name().clone();
    if req.authorities.iter().any(|r| !zone.zone_of(&r.name)) {
        return ResponseCode::NotZone;
    }
    for r in &req.authorities {
        let same = |(x, _): &(Record, Instant)| x.name == r.name && x.data == r.data;
        match r.dns_class {
            DNSClass::IN if !s.records.iter().any(same) => {
                s.records.push((r.clone(), Instant::now()));
            }
            DNSClass::NONE => s.records.retain(|x| !same(x)),
            _ => {}
        }
    }
    ResponseCode::NoError
}

pub struct World {
    pub resolver: Fake,
    pub ns: Vec<Fake>,
    pub reach: super::wire::Reach,
}

impl World {
    pub async fn start(zone: &str, nameservers: usize) -> World {
        let resolver = Fake::start(false).await;
        resolver.add(soa(zone));
        let mut reach = super::wire::Reach::default();
        let mut ns = Vec::new();
        for i in 1..=nameservers {
            let host = format!("ns{i}.dns-host.test");
            let ip = format!("192.0.2.{i}");
            resolver.add(super::fake::ns(zone, &host));
            resolver.add(a(&host, &ip));
            let auth = Fake::start(true).await;
            auth.add(soa(zone));
            reach.redirect.insert(ip.parse().unwrap(), auth.addr);
            ns.push(auth);
        }
        World {
            resolver,
            ns,
            reach,
        }
    }

    pub fn resolvers(&self) -> Vec<SocketAddr> {
        vec![self.resolver.addr]
    }
}

fn matches(owner: &Name, asked: &Name) -> bool {
    if owner == asked {
        return true;
    }
    let base = owner.base_name();
    owner.is_wildcard() && base != *asked && base.zone_of(asked)
}
