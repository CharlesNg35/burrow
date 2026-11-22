//! Tells the admin who asked for an update how it ended, once, as a toast. A finished update leaves
//! no line on the page; only a failure stays, for a day, where the update is offered.

use burrow_admin_proto::updates::{UpdateState, UpdateStatus};
use burrow_ui_kit::Tone;
use leptos::prelude::*;

use crate::store::{OwnJob, store};

/// The job after this status, and the toast when it just ended.
fn step(
    job: OwnJob,
    last: &UpdateStatus,
    client: Option<&str>,
) -> (OwnJob, Option<(Tone, String)>) {
    match (job, last.state.finished()) {
        (OwnJob::Asked, false) => (OwnJob::Running, None),
        (OwnJob::Running, true) => (OwnJob::Idle, Some(ended(last, client))),
        _ => (job, None),
    }
}

fn ended(last: &UpdateStatus, client: Option<&str>) -> (Tone, String) {
    let attention = "see Needs attention";
    match (last.state, client) {
        (UpdateState::Done, None) => (Tone::Success, format!("Server updated to {}", last.to)),
        (UpdateState::Done, Some(c)) => (Tone::Success, format!("{c} updated to {}", last.to)),
        (UpdateState::RolledBack, None) => {
            (Tone::Warning, format!("Update rolled back — {attention}"))
        }
        (UpdateState::RolledBack, Some(c)) => (
            Tone::Warning,
            format!("{c}: update rolled back — {attention}"),
        ),
        (_, None) => (Tone::Warning, format!("Update failed — {attention}")),
        (_, Some(c)) => (Tone::Warning, format!("{c}: update failed — {attention}")),
    }
}

pub fn follow_server_update() {
    let s = store();
    Effect::new(move |_| {
        let job = s.server_updating.get();
        if !job.busy() {
            return;
        }
        let Some(last) = s.status.with(|st| {
            st.as_deref()
                .and_then(|st| st.server_update.as_ref())
                .and_then(|u| u.last.clone())
        }) else {
            return;
        };
        let (next, toast) = step(job, &last, None);
        if next != job {
            s.server_updating.set(next);
        }
        if let Some((tone, text)) = toast {
            s.toast(tone, text, None);
        }
    });
}

/// Clients asked one by one (not with Update all, whose progress has its own line).
pub fn follow_client_updates() {
    let s = store();
    Effect::new(move |_| {
        let jobs = s.clients_updating.get();
        if jobs.is_empty() {
            return;
        }
        let mut next = jobs.clone();
        let mut toasts = Vec::new();
        s.status.with(|st| {
            for (name, job) in &jobs {
                let last = st
                    .as_deref()
                    .and_then(|st| st.clients.iter().find(|c| &c.name == name))
                    .and_then(|c| c.update.as_ref());
                let Some(last) = last else { continue };
                let (job, toast) = step(*job, last, Some(name));
                if job.busy() {
                    next.insert(name.clone(), job);
                } else {
                    next.remove(name);
                }
                toasts.extend(toast);
            }
        });
        if next != jobs {
            s.clients_updating.set(next);
        }
        for (tone, text) in toasts {
            s.toast(tone, text, None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(state: UpdateState) -> UpdateStatus {
        UpdateStatus {
            from: "0.2.4".into(),
            to: "0.2.5".into(),
            state,
            ..UpdateStatus::default()
        }
    }

    #[test]
    fn a_toast_only_when_the_asked_update_ends() {
        let old = last(UpdateState::Done);
        assert_eq!(
            step(OwnJob::Asked, &old, None),
            (OwnJob::Asked, None),
            "the record of an earlier update is not this one's end"
        );
        let busy = last(UpdateState::Downloading);
        assert_eq!(step(OwnJob::Asked, &busy, None), (OwnJob::Running, None));
        assert_eq!(
            step(OwnJob::Running, &old, Some("nas")),
            (
                OwnJob::Idle,
                Some((Tone::Success, "nas updated to 0.2.5".into()))
            )
        );
        assert_eq!(
            step(OwnJob::Running, &last(UpdateState::RolledBack), None).1,
            Some((
                Tone::Warning,
                "Update rolled back — see Needs attention".into()
            ))
        );
        assert_eq!(step(OwnJob::Idle, &old, None), (OwnJob::Idle, None));
    }
}
