use std::sync::Arc;
use std::time::Duration;

use burrow_admin_proto::frame::{self, Assembler, Frame};
use burrow_admin_proto::msg::{self, ConfigEvent, ErrorCode, Request, Revoked, Subscribe};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::debug;

use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::{self, ApiError, Session};
use crate::admin::{Hub, MAX_INFLIGHT, Seat};
use crate::app::reload::{Nudge, ReloadOutcome};
use crate::server::ApplyReport;
use crate::wire::transport::Channel;

const IDLE: Duration = Duration::from_secs(30 * 60);
const RELOAD_WAIT: Duration = Duration::from_secs(15);
const FILE_CHECK: Duration = Duration::from_secs(3);

pub trait Access: Send + Sync {
    fn revoked(&self) -> Option<String>;
}

struct Ctx {
    hub: Arc<Hub>,
    session: Arc<Session>,
    applies: watch::Receiver<ApplyReport>,
    access: Arc<dyn Access>,
}

fn encode(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

fn reply(id: u32, result: Result<Value, ApiError>) -> Vec<u8> {
    let bytes = encode(&match result {
        Ok(body) => msg::reply_ok(id, body),
        Err(e) => msg::reply_err(id, &e.wire()),
    });
    if bytes.len() <= frame::MAX_MESSAGE {
        return bytes;
    }
    debug!("admin reply {id} is {} bytes, over the limit", bytes.len());
    let too_large = ApiError::new(ErrorCode::TooLarge, frame::FrameError::TooLarge.to_string());
    encode(&msg::reply_err(id, &too_large.wire()))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

async fn nudge(hub: &Hub) -> Option<ReloadOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    hub.nudger().send(Nudge { reply: tx }).await.ok()?;
    tokio::time::timeout(RELOAD_WAIT, rx).await.ok()?.ok()
}

async fn apply(cx: &Ctx, body: Value) -> Result<Value, ApiError> {
    let req: msg::Apply = core::parse(&body)?;
    let Some(_queued) = cx.hub.queue() else {
        return Err(ApiError::new(
            ErrorCode::Busy,
            "too many changes are waiting; try again",
        ));
    };
    let _lock = cx.hub.plan_lock().await;
    if let Some(why) = cx.access.revoked() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("this session lost its admin rights ({why}), so nothing was written"),
        ));
    }
    let mut applies = cx.applies.clone();
    let seq = applies.borrow_and_update().seq;
    let (core_, session) = (cx.hub.core.clone(), cx.session.clone());
    let begun = blocking(move || plan::apply_begin(&core_, &session, &req))
        .await
        .ok_or_else(|| ApiError::new(ErrorCode::Internal, "the apply task failed"))??;
    if !begun.wrote {
        let (core_, session) = (cx.hub.core.clone(), cx.session.clone());
        let applied = blocking(move || {
            plan::finish(&core_, &session, begun, ReloadResult::unchanged(), None)
        })
        .await
        .ok_or_else(|| ApiError::new(ErrorCode::Internal, "the apply task failed"))??;
        return core::ok(applied);
    }
    cx.hub
        .wrote(&begun.computed.plan.result_rev, &cx.session.visitor);
    let outcome = nudge(&cx.hub).await;
    let reload = match outcome {
        Some(ReloadOutcome::Rejected(why)) => ReloadResult::Rejected(why),
        Some(ReloadOutcome::Sent) => {
            let waited = tokio::time::timeout(RELOAD_WAIT, applies.wait_for(|r| r.seq > seq)).await;
            match waited {
                Ok(Ok(r)) => match &r.rejected {
                    Some(why) => ReloadResult::Rejected(why.clone()),
                    None => ReloadResult::Applied {
                        restart: r.restart.iter().map(|s| s.to_string()).collect(),
                        unbound: r.unbound.clone(),
                        lines: r.lines.clone(),
                    },
                },
                _ => ReloadResult::Timeout,
            }
        }
        Some(ReloadOutcome::Unchanged) => ReloadResult::unchanged(),
        None => ReloadResult::Timeout,
    };
    let rejected = matches!(reload, ReloadResult::Rejected(_));
    let (core_, session) = (cx.hub.core.clone(), cx.session.clone());
    let finished = blocking(move || {
        let restored = rejected.then(|| plan::restore(&core_, &begun));
        plan::finish(&core_, &session, begun, reload, restored)
    })
    .await
    .ok_or_else(|| ApiError::new(ErrorCode::Internal, "the apply task failed"))?;
    if let Err(e) = &finished
        && e.body["restored"] == Value::Bool(true)
    {
        let _ = nudge(&cx.hub).await;
    }
    let applied = finished?;
    core::ok(applied)
}

async fn handle(cx: Arc<Ctx>, req: Request) -> Vec<u8> {
    let id = req.id;
    let result = match req.t.as_str() {
        msg::req::APPLY => apply(&cx, req.body).await,
        msg::req::VALIDATE | msg::req::REVERT => match cx.hub.queue() {
            None => Err(ApiError::new(
                ErrorCode::Busy,
                "too many changes are waiting; try again",
            )),
            Some(_queued) => {
                let _lock = cx.hub.plan_lock().await;
                let (core_, session) = (cx.hub.core.clone(), cx.session.clone());
                blocking(move || core::dispatch(&core_, &session, &req.t, &req.body))
                    .await
                    .unwrap_or_else(|| {
                        Err(ApiError::new(ErrorCode::Internal, "the request failed"))
                    })
            }
        },
        _ => {
            let (core_, session) = (cx.hub.core.clone(), cx.session.clone());
            blocking(move || core::dispatch(&core_, &session, &req.t, &req.body))
                .await
                .unwrap_or_else(|| Err(ApiError::new(ErrorCode::Internal, "the request failed")))
        }
    };
    reply(id, result)
}

async fn writer_task(
    mut rx: mpsc::Receiver<Vec<u8>>,
    mut writer: crate::wire::record::RecordWriter,
    mut wr: crate::wire::transport::OwnedWriteHalf,
    ping: Duration,
) {
    use tokio::io::AsyncWriteExt;
    loop {
        let next = tokio::time::timeout(ping, rx.recv()).await;
        let records = match next {
            Err(_) => vec![frame::ping().to_vec()],
            Ok(Some(m)) => match frame::chunks(&m) {
                Ok(r) => r,
                Err(e) => {
                    debug!("admin message of {} bytes dropped: {e}", m.len());
                    continue;
                }
            },
            Ok(None) => break,
        };
        for r in records {
            if writer.send(&mut wr, &r).await.is_err() {
                return;
            }
        }
    }
    let _ = writer.send(&mut wr, &[]).await;
    let _ = wr.shutdown().await;
}

fn file_rev(hub: &Hub) -> Option<String> {
    hub.core.store.read().ok().map(|d| core::text::docs_rev(&d))
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    hub: Arc<Hub>,
    seat: Seat,
    channel: Channel,
    session: Session,
    access: Box<dyn Access>,
    mut reloads: watch::Receiver<u64>,
    applies: watch::Receiver<ApplyReport>,
    mut shutdown: watch::Receiver<bool>,
) {
    let _seat = seat;
    let Channel {
        stream,
        writer,
        mut reader,
    } = channel;
    let (mut rd, wr) = stream.into_split();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
    let (ping, silence) = super::keepalive();
    let writer = tokio::spawn(writer_task(rx, writer, wr, ping));
    let visitor = session.visitor.clone();
    let access: Arc<dyn Access> = Arc::from(access);
    let cx = Arc::new(Ctx {
        hub: hub.clone(),
        session: Arc::new(session),
        applies,
        access: access.clone(),
    });
    let mut assembler = Assembler::new();
    let mut jobs: JoinSet<Vec<u8>> = JoinSet::new();
    let mut status_every: Option<tokio::time::Interval> = None;
    let mut config_events = false;
    let mut last_rev = blocking({
        let hub = hub.clone();
        move || file_rev(&hub)
    })
    .await
    .flatten();
    let mut file_check = tokio::time::interval(FILE_CHECK);
    file_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_request = tokio::time::Instant::now();
    let mut heard = tokio::time::Instant::now();
    let send = |bytes: Vec<u8>, until: tokio::time::Instant| {
        let tx = tx.clone();
        async move {
            matches!(
                tokio::time::timeout_at(until, tx.send(bytes)).await,
                Ok(Ok(()))
            )
        }
    };
    let silent = format!("silent for {} s", silence.as_secs());
    let end = loop {
        let idle_left = IDLE.saturating_sub(last_request.elapsed());
        tokio::select! {
            record = reader.recv(&mut rd) => {
                let record = match record {
                    Err(_) | Ok(None) => break "closed",
                    Ok(Some(r)) => r.to_vec(),
                };
                heard = tokio::time::Instant::now();
                if record.is_empty() {
                    break "closed";
                }
                let message = match assembler.push(&record) {
                    Ok(Frame::Message(m)) => m,
                    Ok(_) => continue,
                    Err(e) => {
                        debug!("admin session of visitor \"{visitor}\": {e}");
                        break "protocol error";
                    }
                };
                last_request = tokio::time::Instant::now();
                let Ok(req) = serde_json::from_slice::<Request>(&message) else {
                    break "unreadable request";
                };
                if req.t == msg::req::SUBSCRIBE {
                    let sub: Subscribe = core::parse(&req.body).unwrap_or_default();
                    status_every = status_interval(&sub);
                    config_events = sub.config;
                    if !send(reply(req.id, Ok(serde_json::json!({}))), heard + silence).await {
                        break "closed";
                    }
                    continue;
                }
                if jobs.len() >= MAX_INFLIGHT {
                    let e = ApiError::new(ErrorCode::Busy, "too many requests in flight");
                    if !send(reply(req.id, Err(e)), heard + silence).await {
                        break "closed";
                    }
                    continue;
                }
                spawn_job(&mut jobs, &cx, req);
            }
            Some(done) = jobs.join_next(), if !jobs.is_empty() => {
                if let Ok(v) = done
                    && !send(v, heard + silence).await
                {
                    break "closed";
                }
            }
            _ = async {
                match status_every.as_mut() {
                    Some(i) => { i.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                let status = crate::admin::hooks::status(cx.hub.core.status());
                let ev = msg::event(msg::ev::STATUS, serde_json::to_value(status).unwrap_or_default());
                let _ = tx.try_send(encode(&ev));
            }
            changed = reloads.changed() => {
                if changed.is_err() {
                    break "server stopping";
                }
                if let Some(reason) = access.revoked() {
                    let ev = msg::event(msg::ev::REVOKED, serde_json::to_value(Revoked { reason }).unwrap_or_default());
                    let _ = send(encode(&ev), heard + silence).await;
                    break "revoked";
                }
                if config_events
                    && let Some(ev) = config_event(&hub, &mut last_rev).await
                    && !send(encode(&ev), heard + silence).await
                {
                    break "closed";
                }
            }
            _ = file_check.tick(), if config_events => {
                if let Some(ev) = config_event(&hub, &mut last_rev).await
                    && !send(encode(&ev), heard + silence).await
                {
                    break "closed";
                }
            }
            _ = shutdown.changed() => {
                let _ = send(encode(&msg::event(msg::ev::BYE, serde_json::json!({}))), heard + silence).await;
                break "server stopping";
            }
            _ = tokio::time::sleep_until(heard + silence) => {
                break silent.as_str();
            }
            _ = tokio::time::sleep(idle_left), if status_every.is_none() && jobs.is_empty() => {
                break "idle for 30 min";
            }
        }
    };
    debug!("admin session of visitor \"{visitor}\" ended: {end}");
    jobs.abort_all();
    drop(tx);
    stop_writer(writer).await;
}

fn status_interval(sub: &Subscribe) -> Option<tokio::time::Interval> {
    (sub.status_ms > 0).then(|| {
        let ms = sub.status_ms.clamp(msg::MIN_STATUS_MS, msg::MAX_STATUS_MS);
        let mut i = tokio::time::interval(Duration::from_millis(ms));
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        i
    })
}

fn spawn_job(jobs: &mut JoinSet<Vec<u8>>, cx: &Arc<Ctx>, req: Request) {
    if req.t == msg::req::APPLY {
        let id = req.id;
        let detached = tokio::spawn(handle(cx.clone(), req));
        jobs.spawn(async move {
            detached.await.unwrap_or_else(|_| {
                reply(
                    id,
                    Err(ApiError::new(ErrorCode::Internal, "the apply task failed")),
                )
            })
        });
    } else {
        jobs.spawn(handle(cx.clone(), req));
    }
}

async fn config_event(hub: &Arc<Hub>, last_rev: &mut Option<String>) -> Option<Value> {
    let rev = blocking({
        let hub = hub.clone();
        move || file_rev(&hub)
    })
    .await
    .flatten();
    if rev.is_some() && rev != *last_rev {
        *last_rev = rev.clone();
        let rev = rev.unwrap_or_default();
        let by = hub.written_by(&rev).unwrap_or_else(|| "file".into());
        let ok = blocking({
            let hub = hub.clone();
            move || {
                hub.core
                    .docs()
                    .map(|d| plan::check_docs(&hub.core, &d).valid)
                    .unwrap_or(false)
            }
        })
        .await
        .unwrap_or(false);
        return Some(msg::event(
            msg::ev::CONFIG,
            serde_json::to_value(ConfigEvent { rev, by, ok }).unwrap_or_default(),
        ));
    }
    None
}

async fn stop_writer(mut writer: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(Duration::from_secs(5), &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::core::tests::{core_for, session, world};
    use burrow_admin_proto::msg::AdminLevel;
    use burrow_admin_proto::ops::Op;

    struct RevokedWhileWaiting;

    impl Access for RevokedWhileWaiting {
        fn revoked(&self) -> Option<String> {
            Some("admin level changed".into())
        }
    }

    #[test]
    fn a_reply_over_the_message_limit_is_a_too_large_error() {
        let big = serde_json::json!({ "x": "a".repeat(frame::MAX_MESSAGE) });
        let bytes = reply(7, Ok(big));
        assert!(bytes.len() <= frame::MAX_MESSAGE);
        let text = std::str::from_utf8(&bytes).unwrap();
        match msg::Incoming::parse(text) {
            Some(msg::Incoming::Reply {
                re: 7,
                result: Err(e),
            }) => {
                assert_eq!(e.code, ErrorCode::TooLarge);
            }
            _ => panic!("{text}"),
        }
        let small = reply(8, Ok(serde_json::json!({ "x": 1 })));
        assert!(matches!(
            msg::Incoming::parse(std::str::from_utf8(&small).unwrap()),
            Some(msg::Incoming::Reply {
                re: 8,
                result: Ok(_)
            })
        ));
    }

    #[tokio::test]
    async fn an_apply_of_a_session_revoked_while_it_waited_writes_nothing() {
        let w = world();
        let (nudges, gone) = mpsc::channel(4);
        drop(gone);
        let hub = Hub::new(core_for(&w.text), nudges);
        let s = session(&w, AdminLevel::Write);
        let ops = vec![Op::Set {
            path: vec!["services".into(), "web".into(), "target".into()],
            value: serde_json::json!(3009),
        }];
        let before = hub.core.read().unwrap();
        let base = core::text::rev(&before);
        let plan = plan::compute(&hub.core, &s, &base, &ops).unwrap().plan;
        let (_report, applies) = watch::channel(ApplyReport::default());
        let cx = Ctx {
            hub: hub.clone(),
            session: Arc::new(s),
            applies,
            access: Arc::new(RevokedWhileWaiting),
        };
        let body = serde_json::to_value(msg::Apply {
            base,
            ops,
            plan: plan.plan,
            confirm: Vec::new(),
            address: None,
        })
        .unwrap();
        let e = apply(&cx, body).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::Forbidden, "{}", e.message);
        assert!(e.message.contains("admin level changed"), "{}", e.message);
        assert_eq!(hub.core.read().unwrap(), before, "nothing is written");
    }
}
