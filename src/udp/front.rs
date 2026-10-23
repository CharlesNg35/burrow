use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::UdpSocket;

use super::link::Link;
use super::peers::{AbortOnDrop, PeerCount, PeerGauge, PeerTable};
use crate::protocol::{MAX_DATAGRAM, decode_datagram, encode_datagram};

pub type Peers = Arc<Mutex<PeerTable<SocketAddr>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Done,
    Broken,
}

pub enum Interrupt {
    Retain(Box<dyn Fn(SocketAddr) -> bool + Send>),
    End(Ending),
}

pub trait Meter: Send + Sync + 'static {
    fn take_in(&self, n: usize) -> bool;
    fn count_in(&self, n: usize);
    fn new_peer(&self);
    fn take_out(&self, n: usize) -> bool;
    fn count_out(&self, n: usize);
}

pub trait Hooks {
    fn admit(&self, from: SocketAddr) -> bool;
    fn max_peers(&self) -> usize;
    fn timeout(&self) -> Duration;
    fn full(&self, from: SocketAddr);
    fn stalled(&self, stall: Duration);
    fn recv_error(&self, e: &std::io::Error);
    fn oversized(&self, from: SocketAddr, n: usize) {
        too_large(from, n);
    }
    fn lost(&self) -> impl Future<Output = ()>;
    fn interrupt(&mut self) -> impl Future<Output = Interrupt>;
}

pub fn too_large(from: SocketAddr, n: usize) {
    static LARGE: crate::throttle::Throttle = crate::throttle::Throttle::new();
    if let Some(more) = LARGE.allow() {
        tracing::debug!(
            "dropped a UDP datagram of {n} bytes from {from}: the tunnel carries at most {MAX_DATAGRAM} bytes per datagram{more}"
        );
    }
}

pub struct Front<C: PeerCount, M: Meter> {
    pub socket: Arc<UdpSocket>,
    pub peers: Peers,
    pub gauge: PeerGauge<C>,
    pub meter: Arc<M>,
    pub stall: Duration,
}

pub async fn run<C: PeerCount, M: Meter, H: Hooks>(
    front: &mut Front<C, M>,
    link: Link,
    hooks: &mut H,
) -> Ending {
    let Link {
        mut reader,
        mut rd,
        mut writer,
        mut wr,
    } = link;
    let back_peers = front.peers.clone();
    let back_socket = front.socket.clone();
    let back_meter = front.meter.clone();
    let back = tokio::spawn(async move {
        loop {
            match reader.recv(&mut rd).await {
                Ok(Some(rec)) => {
                    let Ok((id, payload)) = decode_datagram(rec) else {
                        return;
                    };
                    if !back_meter.take_out(payload.len()) {
                        continue;
                    }
                    let addr = back_peers.lock().expect("peers lock").touch(id);
                    if let Some(addr) = addr {
                        back_meter.count_out(payload.len());
                        let _ = back_socket.send_to(payload, addr).await;
                    }
                }
                _ => return,
            }
        }
    });

    let mut back = AbortOnDrop(back);
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    let mut buf = vec![0u8; 65536];
    let mut rec = Vec::with_capacity(65536);
    let stall = front.stall;
    loop {
        tokio::select! {
            r = front.socket.recv_from(&mut buf) => {
                let (n, from) = match r {
                    Ok(v) => v,
                    Err(e) => {
                        hooks.recv_error(&e);
                        continue;
                    }
                };
                if n > MAX_DATAGRAM {
                    hooks.oversized(from, n);
                    continue;
                }
                if !hooks.admit(from) {
                    continue;
                }
                if !front.meter.take_in(n) {
                    continue;
                }
                front.meter.count_in(n);
                let (id, count, before) = {
                    let mut p = front.peers.lock().expect("peers lock");
                    let before = p.len();
                    (p.id_for(from, || hooks.max_peers()), p.len(), before)
                };
                if count > before {
                    front.meter.new_peer();
                }
                front.gauge.set(count);
                let Some(id) = id else {
                    hooks.full(from);
                    continue;
                };
                encode_datagram(id, &buf[..n], &mut rec);
                let sent = tokio::select! {
                    r = tokio::time::timeout(stall, writer.send(&mut wr, &rec)) => r,
                    _ = hooks.lost() => break Ending::Broken,
                };
                match sent {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => break Ending::Broken,
                    Err(_) => {
                        hooks.stalled(stall);
                        break Ending::Broken;
                    }
                }
            }
            _ = sweep.tick() => {
                let count = {
                    let mut p = front.peers.lock().expect("peers lock");
                    p.expire(hooks.timeout());
                    p.len()
                };
                front.gauge.set(count);
            }
            _ = &mut back.0 => break Ending::Broken,
            i = hooks.interrupt() => match i {
                Interrupt::End(ending) => break ending,
                Interrupt::Retain(keep) => {
                    let count = {
                        let mut p = front.peers.lock().expect("peers lock");
                        p.retain(keep);
                        p.len()
                    };
                    front.gauge.set(count);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::local::pair;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Count(Arc<AtomicUsize>);

    impl PeerCount for Count {
        fn peers(&self) -> &AtomicUsize {
            &self.0
        }
    }

    #[derive(Default)]
    struct Tally {
        inbound: AtomicUsize,
        outbound: AtomicUsize,
        peers: AtomicUsize,
    }

    impl Meter for Tally {
        fn take_in(&self, _: usize) -> bool {
            true
        }
        fn count_in(&self, n: usize) {
            self.inbound.fetch_add(n, Ordering::Relaxed);
        }
        fn new_peer(&self) {
            self.peers.fetch_add(1, Ordering::Relaxed);
        }
        fn take_out(&self, _: usize) -> bool {
            true
        }
        fn count_out(&self, n: usize) {
            self.outbound.fetch_add(n, Ordering::Relaxed);
        }
    }

    struct Test {
        refused: SocketAddr,
        end: tokio::sync::mpsc::Receiver<Interrupt>,
        big: Arc<AtomicUsize>,
    }

    impl Hooks for Test {
        fn admit(&self, from: SocketAddr) -> bool {
            from != self.refused
        }
        fn max_peers(&self) -> usize {
            1
        }
        fn timeout(&self) -> Duration {
            Duration::from_secs(60)
        }
        fn full(&self, _: SocketAddr) {}
        fn stalled(&self, _: Duration) {}
        fn recv_error(&self, _: &std::io::Error) {}
        fn oversized(&self, _: SocketAddr, n: usize) {
            self.big.store(n, Ordering::Release);
        }
        async fn lost(&self) {
            std::future::pending::<()>().await
        }
        async fn interrupt(&mut self) -> Interrupt {
            match self.end.recv().await {
                Some(i) => i,
                None => std::future::pending().await,
            }
        }
    }

    #[tokio::test]
    async fn datagrams_cross_with_peer_ids_and_only_admitted_capped_senders_get_in() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let at = socket.local_addr().unwrap();
        let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let refused = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let extra = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (near, mut far) = pair();
        let crate::handshake::Channel {
            stream,
            writer,
            reader,
        } = near;
        let (rd, wr) = stream.into_split();
        let link = Link {
            reader,
            rd,
            writer,
            wr,
        };
        let total = Arc::new(AtomicUsize::new(0));
        let meter = Arc::new(Tally::default());
        let mut front = Front {
            socket,
            peers: Arc::new(Mutex::new(PeerTable::default())),
            gauge: PeerGauge::new(Count(total.clone())),
            meter: meter.clone(),
            stall: Duration::from_secs(5),
        };
        let (tx, end) = tokio::sync::mpsc::channel(1);
        let mut hooks = Test {
            refused: refused.local_addr().unwrap(),
            end,
            big: Arc::new(AtomicUsize::new(0)),
        };
        let running = tokio::spawn(async move {
            let ending = run(&mut front, link, &mut hooks).await;
            (ending, front)
        });
        refused.send_to(b"no", at).await.unwrap();
        first.send_to(b"hello", at).await.unwrap();
        let rec = far
            .reader
            .recv(&mut far.stream)
            .await
            .unwrap()
            .unwrap()
            .to_vec();
        let (id, payload) = decode_datagram(&rec).unwrap();
        assert_eq!(payload, b"hello");
        extra.send_to(b"over the cap", at).await.unwrap();
        first.send_to(b"again", at).await.unwrap();
        let rec = far
            .reader
            .recv(&mut far.stream)
            .await
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(decode_datagram(&rec).unwrap(), (id, &b"again"[..]));
        let mut reply = Vec::new();
        encode_datagram(id, b"back", &mut reply);
        far.writer.send(&mut far.stream, &reply).await.unwrap();
        let mut buf = [0u8; 16];
        let n = first.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"back");
        assert_eq!(total.load(Ordering::Acquire), 1);
        tx.send(Interrupt::Retain(Box::new(|_| false)))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while total.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tx.send(Interrupt::End(Ending::Done)).await.unwrap();
        let (ending, front) = running.await.unwrap();
        assert_eq!(ending, Ending::Done);
        assert!(front.peers.lock().unwrap().is_empty());
        assert_eq!(meter.peers.load(Ordering::Relaxed), 1);
        assert_eq!(meter.inbound.load(Ordering::Relaxed), 5 + 12 + 5);
        assert_eq!(meter.outbound.load(Ordering::Relaxed), 4);
        drop(front);
        assert_eq!(total.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_broken_link_ends_the_front_as_broken() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (near, far) = pair();
        let crate::handshake::Channel {
            stream,
            writer,
            reader,
        } = near;
        let (rd, wr) = stream.into_split();
        let link = Link {
            reader,
            rd,
            writer,
            wr,
        };
        let mut front = Front {
            socket,
            peers: Arc::new(Mutex::new(PeerTable::default())),
            gauge: PeerGauge::new(Count(Arc::new(AtomicUsize::new(0)))),
            meter: Arc::new(Tally::default()),
            stall: Duration::from_secs(5),
        };
        let (_tx, end) = tokio::sync::mpsc::channel(1);
        let mut hooks = Test {
            refused: "127.0.0.1:1".parse().unwrap(),
            end,
            big: Arc::new(AtomicUsize::new(0)),
        };
        drop(far);
        let ending =
            tokio::time::timeout(Duration::from_secs(5), run(&mut front, link, &mut hooks))
                .await
                .unwrap();
        assert_eq!(ending, Ending::Broken);
    }

    #[tokio::test]
    async fn a_datagram_too_large_for_one_record_is_reported_and_dropped() {
        let Ok(socket) = UdpSocket::bind("[::1]:0").await else {
            return;
        };
        let socket = Arc::new(socket);
        let at = socket.local_addr().unwrap();
        let sender = UdpSocket::bind("[::1]:0").await.unwrap();
        let (near, mut far) = pair();
        let crate::handshake::Channel {
            stream,
            writer,
            reader,
        } = near;
        let (rd, wr) = stream.into_split();
        let link = Link {
            reader,
            rd,
            writer,
            wr,
        };
        let mut front = Front {
            socket,
            peers: Arc::new(Mutex::new(PeerTable::default())),
            gauge: PeerGauge::new(Count(Arc::new(AtomicUsize::new(0)))),
            meter: Arc::new(Tally::default()),
            stall: Duration::from_secs(5),
        };
        let (_tx, end) = tokio::sync::mpsc::channel(1);
        let big = Arc::new(AtomicUsize::new(0));
        let mut hooks = Test {
            refused: "[::1]:1".parse().unwrap(),
            end,
            big: big.clone(),
        };
        let running = tokio::spawn(async move { run(&mut front, link, &mut hooks).await });
        let too_big = vec![1u8; MAX_DATAGRAM + 5];
        if sender.send_to(&too_big, at).await.is_err() {
            running.abort();
            return;
        }
        sender.send_to(b"small", at).await.unwrap();
        let rec = far
            .reader
            .recv(&mut far.stream)
            .await
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(decode_datagram(&rec).unwrap().1, b"small");
        assert_eq!(big.load(Ordering::Acquire), MAX_DATAGRAM + 5);
        running.abort();
    }
}
