use burrow_admin_proto::msg::State;
use burrow_admin_proto::status::Status;

pub fn differences(state: &State, status: Option<&Status>) -> usize {
    let file = usize::from(!state.running.ok)
        + state.runtime.restart_pending.len()
        + state.runtime.not_open.len();
    let live = status.map_or(0, |st| {
        st.services
            .iter()
            .filter(|s| s.not_listening.is_some())
            .count()
            + usize::from(st.web_waiting.is_some())
    });
    file + live
}

pub fn difference_note(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("The running server differs from this file in 1 place.".into()),
        n => Some(format!(
            "The running server differs from this file in {n} places."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::msg::NotOpen;
    use burrow_admin_proto::status::{PortWait, ServiceStatus};

    fn healthy() -> State {
        let mut st = State::default();
        st.running.ok = true;
        st
    }

    #[test]
    fn a_server_that_runs_its_file_shows_no_note() {
        let status = Status {
            services: vec![ServiceStatus {
                name: "web".into(),
                ..ServiceStatus::default()
            }],
            ..Status::default()
        };
        assert_eq!(differences(&healthy(), Some(&status)), 0);
        assert_eq!(differences(&healthy(), None), 0);
        assert_eq!(difference_note(0), None);
    }

    #[test]
    fn each_way_the_running_server_differs_counts_once() {
        let mut bad = healthy();
        bad.running.ok = false;
        assert_eq!(differences(&bad, None), 1);
        let mut restart = healthy();
        restart.runtime.restart_pending = vec!["listen".into()];
        assert_eq!(differences(&restart, None), 1);
        let mut closed = healthy();
        closed.runtime.not_open = vec![NotOpen::default(), NotOpen::default()];
        assert_eq!(differences(&closed, None), 2);
        let status = Status {
            services: vec![ServiceStatus {
                name: "web".into(),
                not_listening: Some("in use".into()),
                ..ServiceStatus::default()
            }],
            web_waiting: Some(PortWait::default()),
            ..Status::default()
        };
        assert_eq!(differences(&healthy(), Some(&status)), 2);
        assert_eq!(
            difference_note(1).as_deref(),
            Some("The running server differs from this file in 1 place.")
        );
        assert!(difference_note(3).unwrap().contains("in 3 places"));
    }
}
