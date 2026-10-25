use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use burrow_admin_proto::msg::Apply;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Applied;
use serde_json::Value;

pub const MAX_KEPT: usize = 16;
pub const KEEP_FOR: Duration = Duration::from_secs(30 * 60);

struct Kept {
    plan: String,
    base: String,
    ops: Vec<Op>,
    at: Instant,
}

#[derive(Default)]
pub struct Plans {
    kept: Mutex<VecDeque<Kept>>,
}

impl Plans {
    pub fn keep(&self, plan: &str, base: &str, ops: Vec<Op>) {
        self.keep_at(plan, base, ops, Instant::now());
    }

    fn keep_at(&self, plan: &str, base: &str, ops: Vec<Op>, now: Instant) {
        let Ok(mut kept) = self.kept.lock() else {
            return;
        };
        kept.retain(|k| k.plan != plan && now.duration_since(k.at) < KEEP_FOR);
        while kept.len() >= MAX_KEPT {
            kept.pop_front();
        }
        kept.push_back(Kept {
            plan: plan.to_string(),
            base: base.to_string(),
            ops,
            at: now,
        });
    }

    pub fn apply(&self, plan: &str, confirm: Vec<String>) -> Option<Apply> {
        self.apply_at(plan, confirm, Instant::now())
    }

    fn apply_at(&self, plan: &str, confirm: Vec<String>, now: Instant) -> Option<Apply> {
        let mut kept = self.kept.lock().ok()?;
        kept.retain(|k| now.duration_since(k.at) < KEEP_FOR);
        let k = kept.iter().find(|k| k.plan == plan)?;
        Some(Apply {
            base: k.base.clone(),
            ops: k.ops.clone(),
            plan: k.plan.clone(),
            confirm,
            address: None,
        })
    }

    pub fn forget(&self, plan: &str) {
        if let Ok(mut kept) = self.kept.lock() {
            kept.retain(|k| k.plan != plan);
        }
    }
}

pub fn without_secrets(applied: Value) -> Value {
    let Ok(mut a) = serde_json::from_value::<Applied>(applied) else {
        return serde_json::json!({ "applied": true });
    };
    for s in &mut a.secrets {
        s.value.clear();
    }
    a.snippets.clear();
    serde_json::to_value(a).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(n: u32) -> Vec<Op> {
        vec![Op::Set {
            path: vec!["a".into()],
            value: n.into(),
        }]
    }

    #[test]
    fn a_kept_plan_applies_with_its_own_base_and_ops() {
        let p = Plans::default();
        p.keep("p1", "b1", set(1));
        let a = p.apply("p1", vec!["restart".into()]).unwrap();
        assert_eq!((a.base.as_str(), a.plan.as_str()), ("b1", "p1"));
        assert_eq!(a.ops, set(1));
        assert_eq!(a.confirm, vec!["restart".to_string()]);
        assert!(p.apply("p2", Vec::new()).is_none());
        p.forget("p1");
        assert!(p.apply("p1", Vec::new()).is_none());
    }

    #[test]
    fn the_store_keeps_the_newest_plans_only() {
        let p = Plans::default();
        for n in 0..(MAX_KEPT as u32 + 2) {
            p.keep(&format!("p{n}"), "b", set(n));
        }
        assert!(p.apply("p0", Vec::new()).is_none());
        assert!(p.apply("p1", Vec::new()).is_none());
        assert!(p.apply("p2", Vec::new()).is_some());
        assert_eq!(p.kept.lock().unwrap().len(), MAX_KEPT);
    }

    #[test]
    fn a_plan_older_than_30_minutes_is_gone() {
        let p = Plans::default();
        let then = Instant::now();
        p.keep_at("p1", "b", set(1), then);
        let almost = then + KEEP_FOR - Duration::from_secs(1);
        assert!(p.apply_at("p1", Vec::new(), almost).is_some());
        assert!(p.apply_at("p1", Vec::new(), then + KEEP_FOR).is_none());
    }

    #[test]
    fn an_apply_result_never_carries_a_secret_or_a_snippet() {
        let v = serde_json::json!({
            "rev": "b2s:2",
            "secrets": [{"handle": "h", "path": ["clients", "x", "secret"], "fp": "ab12", "value": "s3cr3t", "shown_once": true}],
            "snippets": [{"kind": "client", "name": "x", "config": "secret: s3cr3t"}]
        });
        let out = without_secrets(v).to_string();
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(out.contains("ab12"));
    }
}
