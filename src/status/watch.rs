use std::collections::{BTreeMap, HashMap};

use burrow_admin_proto::updates::{UpdateState, UpdateStatus, state_text};

use crate::metrics::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: Option<UpdateState>,
    pub detail: String,
}

pub struct Watch {
    to: String,
    asked: Vec<String>,
    before: HashMap<String, u32>,
    said: HashMap<String, UpdateState>,
    pub results: BTreeMap<String, Outcome>,
}

fn line(name: &str, u: &UpdateStatus) -> String {
    match u.state {
        UpdateState::Done => format!("{name}: {} → {} updated", u.from, u.to),
        s if s.failed() && !u.detail.is_empty() => {
            format!("{name}: {}: {}", state_text(s), u.detail)
        }
        s => format!("{name}: {}", state_text(s)),
    }
}

impl Watch {
    pub fn new(to: &str, asked: &[String], before: &Status) -> Watch {
        let before = before
            .clients
            .iter()
            .filter_map(|c| Some((c.name.clone(), c.update.as_ref()?.id)))
            .collect();
        Watch {
            to: to.to_string(),
            asked: asked.to_vec(),
            before,
            said: HashMap::new(),
            results: BTreeMap::new(),
        }
    }

    fn job<'a>(&self, st: &'a Status, name: &str) -> Option<&'a UpdateStatus> {
        let u = st
            .clients
            .iter()
            .find(|c| c.name == name)?
            .update
            .as_ref()?;
        (self.before.get(name) != Some(&u.id) && u.to == self.to).then_some(u)
    }

    pub fn step(&mut self, st: &Status) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        for name in self.asked.clone() {
            let Some(u) = self.job(st, &name).cloned() else {
                continue;
            };
            if self.said.insert(name.clone(), u.state) == Some(u.state) {
                continue;
            }
            lines.push(line(&name, &u));
            if u.state.finished() {
                self.results.insert(
                    name,
                    Outcome {
                        state: Some(u.state),
                        detail: u.detail.clone(),
                    },
                );
            }
        }
        let stopped = self.asked.len() > 1
            && st.rollout.as_ref().is_some_and(|r| {
                r.finished && r.stopped_at.is_some() && r.to == self.to && r.clients == self.asked
            });
        if stopped {
            let left: Vec<String> = self
                .asked
                .iter()
                .filter(|n| !self.results.contains_key(*n))
                .cloned()
                .collect();
            if !left.is_empty() {
                lines.push(format!("stopped: {} not updated", left.join(", ")));
            }
            for n in left {
                self.results.insert(
                    n,
                    Outcome {
                        state: None,
                        detail: "not updated: the update stopped at an earlier client".into(),
                    },
                );
            }
        }
        let done = self.asked.iter().all(|n| self.results.contains_key(n));
        (lines, done)
    }

    pub fn ok(&self) -> bool {
        self.asked.iter().all(|n| {
            self.results
                .get(n)
                .is_some_and(|o| o.state == Some(UpdateState::Done))
        })
    }
}

#[cfg(test)]
mod tests;
