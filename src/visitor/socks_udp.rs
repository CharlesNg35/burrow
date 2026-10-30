use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use super::Live;
use super::forward::{explain, rejected};
use super::socks::{Hint, Reply, reply_bound};
use crate::dialer::DialError;
use crate::policy::throttle::Throttle;
use crate::udp::link::{Opening, open_udp};
use crate::udp::peers::AbortOnDrop;
use crate::wire::protocol::{
    ChannelKind, Hello, VERSION, VisitRequest, VisitTarget, decode_datagram, encode_datagram,
};
use crate::wire::transport::udp::bind_udp_now;
use burrow_config::Protocol;

pub const MAX_DESTINATIONS: usize = 64;
const QUEUE: usize = 64;
pub const QUEUED_BYTES: usize = 1 << 20;

struct Budget {
    used: AtomicUsize,
    cap: usize,
}

impl Budget {
    fn new(cap: usize) -> Budget {
        Budget {
            used: AtomicUsize::new(0),
            cap,
        }
    }
}

struct Queued {
    data: Vec<u8>,
    budget: Arc<Budget>,
}

impl Queued {
    fn new(data: Vec<u8>, budget: &Arc<Budget>) -> Option<Queued> {
        let n = data.len();
        budget
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used + n <= budget.cap).then_some(used + n)
            })
            .ok()?;
        Some(Queued {
            data,
            budget: budget.clone(),
        })
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.budget
            .used
            .fetch_sub(self.data.len(), Ordering::AcqRel);
    }
}
const PEER: u32 = 1;
const RETRY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header<'a> {
    pub target: String,
    pub head: &'a [u8],
    pub data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadHeader {
    Short,
    Reserved,
    Fragment(u8),
    AddressType(u8),
    Name,
    Port,
}

pub fn parse(datagram: &[u8]) -> Result<Header<'_>, BadHeader> {
    let [r1, r2, frag, atyp, rest @ ..] = datagram else {
        return Err(BadHeader::Short);
    };
    if [*r1, *r2] != [0, 0] {
        return Err(BadHeader::Reserved);
    }
    if *frag != 0 {
        return Err(BadHeader::Fragment(*frag));
    }
    let (host, rest) = match *atyp {
        1 => {
            let (ip, rest) = rest.split_first_chunk::<4>().ok_or(BadHeader::Short)?;
            (Ipv4Addr::from(*ip).to_string(), rest)
        }
        4 => {
            let (ip, rest) = rest.split_first_chunk::<16>().ok_or(BadHeader::Short)?;
            (format!("[{}]", Ipv6Addr::from(*ip)), rest)
        }
        3 => {
            let (len, rest) = rest.split_first().ok_or(BadHeader::Short)?;
            let name = rest.get(..usize::from(*len)).ok_or(BadHeader::Short)?;
            let name = std::str::from_utf8(name).map_err(|_| BadHeader::Name)?;
            let host = match name.parse::<Ipv6Addr>() {
                Ok(ip) => format!("[{ip}]"),
                Err(_) => name.to_string(),
            };
            (host, &rest[usize::from(*len)..])
        }
        n => return Err(BadHeader::AddressType(n)),
    };
    let (port, data) = rest.split_first_chunk::<2>().ok_or(BadHeader::Short)?;
    let port = u16::from_be_bytes(*port);
    if port == 0 {
        return Err(BadHeader::Port);
    }
    let target = format!("{host}:{port}");
    if !super::forward::socks_target_ok(&target)
        || host.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(BadHeader::Name);
    }
    Ok(Header {
        target,
        head: &datagram[..datagram.len() - data.len()],
        data,
    })
}

pub(super) struct Association {
    pub name: String,
    pub client: String,
    pub stream: TcpStream,
    pub peer: SocketAddr,
    pub hint: Hint,
    pub live: watch::Receiver<Live>,
}

struct Destination {
    tx: mpsc::Sender<Queued>,
    done: Arc<AtomicBool>,
    seen: tokio::time::Instant,
    ended: Option<tokio::time::Instant>,
    _task: AbortOnDrop,
}

pub(super) async fn associate(a: Association) {
    let Association {
        name,
        client,
        mut stream,
        peer,
        hint,
        live,
    } = a;
    let Ok(local) = stream.local_addr() else {
        return;
    };
    let near = local.ip().to_canonical();
    let relay = match bind_udp_now(SocketAddr::new(near, 0)) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            warn!("forward \"{name}\": cannot open a UDP relay port for a SOCKS app: {e}");
            let _ = reply_bound(&mut stream, Reply::Failure, SocketAddr::new(near, 0)).await;
            return;
        }
    };
    let Ok(bound) = relay.local_addr() else {
        return;
    };
    if reply_bound(&mut stream, Reply::Succeeded, bound)
        .await
        .is_err()
    {
        return;
    }
    debug!(%peer, "forward \"{name}\": UDP ASSOCIATE relay on {bound}");
    if let Some(named) = hint.ip.filter(|ip| !same_ip(*ip, peer.ip())) {
        debug!(%peer, "forward \"{name}\": the SOCKS app named {named} as its UDP source; only its own address {} is admitted", peer.ip());
    }
    let expected = Source::new(hint, peer.ip());
    let mut locked: Option<SocketAddr> = None;
    let mut destinations: HashMap<String, Destination> = HashMap::new();
    let budget = Arc::new(Budget::new(QUEUED_BYTES));
    let mut control = [0u8; 512];
    let mut sweep = tokio::time::interval(crate::udp::front::SWEEP);
    let mut last = tokio::time::Instant::now();
    loop {
        tokio::select! {
            r = stream.read(&mut control) => {
                if matches!(r, Ok(0) | Err(_)) {
                    debug!(%peer, "forward \"{name}\": the SOCKS app closed its UDP association");
                    break;
                }
            }
            r = relay.readable() => {
                if r.is_err() {
                    break;
                }
                let (datagram, from) = match super::udp::receive_from(&relay, &name) {
                    super::udp::Received::Datagram(data, from) => (data, from),
                    super::udp::Received::Nothing => continue,
                    super::udp::Received::Failed => {
                        tokio::time::sleep(super::udp::RECEIVE_ERROR_PAUSE).await;
                        continue;
                    }
                };
                if !expected.admits(from, locked) {
                    static FOREIGN: Throttle = Throttle::new();
                    if let Some(more) = FOREIGN.allow() {
                        debug!("forward \"{name}\": dropped a datagram from {from}, which is not the SOCKS app of this association{more}");
                    }
                    continue;
                }
                locked = Some(from);
                let (target, head_len) = match parse(&datagram) {
                    Ok(h) => (h.target, h.head.len()),
                    Err(e) => {
                        static BAD: Throttle = Throttle::new();
                        if let Some(more) = BAD.allow() {
                            debug!("forward \"{name}\": dropped a SOCKS UDP datagram: {}{more}", e.meaning());
                        }
                        continue;
                    }
                };
                last = tokio::time::Instant::now();
                let now = last;
                let open = destinations.get(&target).is_some_and(|d| {
                    !d.done.load(Ordering::Acquire) || d.ended.is_some_and(|t| now.duration_since(t) < RETRY)
                });
                if !open {
                    destinations.remove(&target);
                    if destinations.len() >= MAX_DESTINATIONS {
                        static FULL: Throttle = Throttle::new();
                        if let Some(more) = FULL.allow() {
                            warn!("forward \"{name}\": a SOCKS app sent UDP to more than {MAX_DESTINATIONS} destinations at once; dropping datagrams to new ones{more}");
                        }
                        continue;
                    }
                    let (tx, rx) = mpsc::channel(QUEUE);
                    let done = Arc::new(AtomicBool::new(false));
                    let task = tokio::spawn(destination(Target {
                        name: name.clone(),
                        to: VisitTarget::Client {
                            client: client.clone(),
                            target: target.clone(),
                        },
                        head: datagram[..head_len].to_vec(),
                        rx,
                        relay: relay.clone(),
                        app: from,
                        live: live.clone(),
                        done: done.clone(),
                    }));
                    destinations.insert(target.clone(), Destination {
                        tx,
                        done,
                        seen: now,
                        ended: None,
                        _task: AbortOnDrop(task),
                    });
                }
                if let Some(d) = destinations.get_mut(&target) {
                    d.seen = now;
                    let mut data = datagram;
                    data.drain(..head_len);
                    match Queued::new(data, &budget) {
                        Some(q) => {
                            let _ = d.tx.try_send(q);
                        }
                        None => {
                            static OVER: Throttle = Throttle::new();
                            if let Some(more) = OVER.allow() {
                                debug!("forward \"{name}\": a SOCKS app has {QUEUED_BYTES} bytes of UDP waiting for the server; dropping a datagram{more}");
                            }
                        }
                    }
                }
            }
            _ = sweep.tick() => {
                let timeout = live.borrow().settings.udp_session_timeout;
                let now = tokio::time::Instant::now();
                for d in destinations.values_mut() {
                    if d.ended.is_none() && d.done.load(Ordering::Acquire) {
                        d.ended = Some(now);
                    }
                }
                destinations.retain(|_, d| now.duration_since(d.seen) < timeout);
                if now.duration_since(last) >= timeout && destinations.is_empty() {
                    debug!(%peer, "forward \"{name}\": UDP association idle for {}s, closing it", timeout.as_secs());
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Source {
    ip: IpAddr,
    port: Option<u16>,
}

impl Source {
    fn new(hint: Hint, peer: IpAddr) -> Source {
        Source {
            ip: peer.to_canonical(),
            port: (hint.port != 0).then_some(hint.port),
        }
    }

    fn admits(&self, from: SocketAddr, locked: Option<SocketAddr>) -> bool {
        if let Some(l) = locked {
            return same_ip(l.ip(), from.ip()) && l.port() == from.port();
        }
        from.ip().to_canonical() == self.ip && self.port.is_none_or(|p| p == from.port())
    }
}

fn same_ip(a: IpAddr, b: IpAddr) -> bool {
    a.to_canonical() == b.to_canonical()
}

impl BadHeader {
    pub fn meaning(self) -> String {
        match self {
            BadHeader::Short => "its header is cut short".into(),
            BadHeader::Reserved => "its reserved bytes are not zero".into(),
            BadHeader::Fragment(n) => {
                format!("it is fragment {n}, and fragments are not supported")
            }
            BadHeader::AddressType(n) => format!("address type {n} is not defined"),
            BadHeader::Name => "its destination is not a valid host:port".into(),
            BadHeader::Port => "its destination port is 0".into(),
        }
    }
}

struct Target {
    name: String,
    to: VisitTarget,
    head: Vec<u8>,
    rx: mpsc::Receiver<Queued>,
    relay: Arc<UdpSocket>,
    app: SocketAddr,
    live: watch::Receiver<Live>,
    done: Arc<AtomicBool>,
}

async fn destination(t: Target) {
    let Target {
        name,
        to,
        head,
        mut rx,
        relay,
        app,
        live,
        done,
    } = t;
    run_destination(&name, &to, &head, &mut rx, &relay, app, &live).await;
    done.store(true, Ordering::Release);
}

async fn run_destination(
    name: &str,
    to: &VisitTarget,
    head: &[u8],
    rx: &mut mpsc::Receiver<Queued>,
    relay: &UdpSocket,
    app: SocketAddr,
    live: &watch::Receiver<Live>,
) {
    let Ok(request) = (VisitRequest {
        check: false,
        protocol: Protocol::Udp,
        to: to.clone(),
    })
    .encode() else {
        return;
    };
    let dialer = live.borrow().dialer.clone();
    let ch = match dialer
        .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            static LOST: Throttle = Throttle::new();
            if let Some(more) = LOST.allow() {
                let why = match e {
                    DialError::Rejected => rejected(&dialer),
                    DialError::Lost(why) | DialError::Unreachable(why) => why,
                };
                warn!("forward \"{name}\": {why}{more}");
            }
            return;
        }
    };
    let link = match open_udp(ch, &request, crate::forward::VISIT_REPLY).await {
        Ok(link) => link,
        Err(failed) => {
            static REFUSED: Throttle = Throttle::new();
            if let Some(more) = REFUSED.allow() {
                let why = match failed {
                    Opening::Refused(status) => explain(status, to, &dialer.name, Protocol::Udp),
                    Opening::Silent => "the server did not answer in time".into(),
                    Opening::Lost => "the server closed the channel before answering".into(),
                };
                info!("forward \"{name}\": UDP from a SOCKS app to {to}: {why}{more}");
            }
            return;
        }
    };
    let crate::udp::link::Link {
        mut reader,
        mut rd,
        mut writer,
        mut wr,
    } = link;
    let stall = crate::forward::VISIT_REPLY;
    let up = async {
        let mut rec = Vec::new();
        let mut sealed = Vec::new();
        while let Some(queued) = rx.recv().await {
            encode_datagram(PEER, &queued.data, &mut rec);
            drop(queued);
            match tokio::time::timeout(stall, writer.send_with(&mut wr, &rec, &mut sealed)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    };
    let down = async {
        let mut out = Vec::new();
        loop {
            let rec = match reader.recv(&mut rd).await {
                Ok(Some(rec)) => rec,
                _ => return,
            };
            let data = match decode_datagram(rec) {
                Ok((PEER, data)) => data,
                Ok(_) => continue,
                Err(_) => return,
            };
            out.clear();
            out.extend_from_slice(head);
            out.extend_from_slice(data);
            let _ = relay.send_to(&out, app).await;
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagram(atyp: u8, addr: &[u8], port: u16, data: &[u8]) -> Vec<u8> {
        let mut d = vec![0, 0, 0, atyp];
        d.extend_from_slice(addr);
        d.extend_from_slice(&port.to_be_bytes());
        d.extend_from_slice(data);
        d
    }

    #[test]
    fn each_address_type_names_its_target_and_keeps_the_header_for_replies() {
        let v4 = datagram(1, &[10, 0, 0, 5], 53, b"query");
        let h = parse(&v4).unwrap();
        assert_eq!(h.target, "10.0.0.5:53");
        assert_eq!(h.head, &v4[..10]);
        assert_eq!(h.data, b"query");
        let v6 = datagram(4, &Ipv6Addr::LOCALHOST.octets(), 53, b"");
        let h = parse(&v6).unwrap();
        assert_eq!(h.target, "[::1]:53");
        assert_eq!(h.head.len(), 22);
        assert!(h.data.is_empty());
        let mut name = vec![9];
        name.extend_from_slice(b"dns.local");
        let named = datagram(3, &name, 5353, b"x");
        let h = parse(&named).unwrap();
        assert_eq!(h.target, "dns.local:5353");
        assert_eq!(h.data, b"x");
        let mut literal = vec![3];
        literal.extend_from_slice(b"::1");
        assert_eq!(
            parse(&datagram(3, &literal, 53, b"")).unwrap().target,
            "[::1]:53"
        );
    }

    #[test]
    fn queued_datagrams_of_an_association_share_one_byte_budget() {
        let budget = Arc::new(Budget::new(1000));
        let a = Queued::new(vec![0; 600], &budget).unwrap();
        assert!(
            Queued::new(vec![0; 600], &budget).is_none(),
            "over the budget"
        );
        let b = Queued::new(vec![0; 400], &budget).unwrap();
        assert!(Queued::new(vec![0; 1], &budget).is_none());
        drop(a);
        let c = Queued::new(vec![0; 600], &budget).unwrap();
        assert_eq!(c.data.len(), 600);
        drop((b, c));
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
        assert_eq!(QUEUED_BYTES, 1 << 20);
    }

    #[test]
    fn only_the_app_of_the_association_is_admitted() {
        let at = |s: &str| s.parse::<SocketAddr>().unwrap();
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        let any = Source::new(Hint { ip: None, port: 0 }, peer);
        assert!(any.admits(at("127.0.0.1:4000"), None));
        assert!(any.admits(at("[::ffff:127.0.0.1]:4000"), None));
        assert!(!any.admits(at("127.0.0.2:4000"), None));
        let locked = Some(at("127.0.0.1:4000"));
        assert!(any.admits(at("127.0.0.1:4000"), locked));
        assert!(!any.admits(at("127.0.0.1:4001"), locked));
        let named = Source::new(
            Hint {
                ip: Some("10.0.0.7".parse().unwrap()),
                port: 5000,
            },
            peer,
        );
        assert!(
            !named.admits(at("10.0.0.7:5000"), None),
            "a named address other than the app's own is never admitted"
        );
        assert!(named.admits(at("127.0.0.1:5000"), None));
        assert!(!named.admits(at("127.0.0.1:5001"), None));
        let own = Source::new(
            Hint {
                ip: Some("::ffff:127.0.0.1".parse().unwrap()),
                port: 0,
            },
            peer,
        );
        assert!(own.admits(at("127.0.0.1:9"), None));
    }

    #[test]
    fn anything_but_a_whole_unfragmented_header_is_refused() {
        assert_eq!(parse(&[]), Err(BadHeader::Short));
        assert_eq!(parse(&[0, 0, 0]), Err(BadHeader::Short));
        let mut reserved = datagram(1, &[1, 2, 3, 4], 53, b"");
        reserved[1] = 1;
        assert_eq!(parse(&reserved), Err(BadHeader::Reserved));
        let mut frag = datagram(1, &[1, 2, 3, 4], 53, b"");
        frag[2] = 0x81;
        assert_eq!(parse(&frag), Err(BadHeader::Fragment(0x81)));
        assert_eq!(
            parse(&datagram(2, &[], 53, b"")),
            Err(BadHeader::AddressType(2))
        );
        assert_eq!(parse(&[0, 0, 0, 1, 1, 2, 3, 4, 0]), Err(BadHeader::Short));
        assert_eq!(parse(&[0, 0, 0, 4, 1, 2, 3]), Err(BadHeader::Short));
        assert_eq!(parse(&[0, 0, 0, 3]), Err(BadHeader::Short));
        assert_eq!(parse(&[0, 0, 0, 3, 5, b'a']), Err(BadHeader::Short));
        assert_eq!(
            parse(&datagram(1, &[1, 2, 3, 4], 0, b"")),
            Err(BadHeader::Port)
        );
        for bad in [
            &b""[..],
            b"a b",
            b"a\nb",
            b"unix:/x",
            b"\xff\xfe",
            b"a:b:c:",
        ] {
            let mut name = vec![bad.len() as u8];
            name.extend_from_slice(bad);
            assert_eq!(
                parse(&datagram(3, &name, 53, b"")),
                Err(BadHeader::Name),
                "{bad:?}"
            );
        }
    }
}
