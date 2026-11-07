#![cfg(test)]

use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use serde_json::json;

use super::super::p;
use super::{BLOG, Set, rename};

fn said(plan: &Plan) -> Vec<String> {
    plan.changes.iter().map(|c| c.summary.clone()).collect()
}

fn remove(name: &str) -> Op {
    Op::Remove {
        path: p(&["services", name]),
    }
}

#[test]
fn renaming_an_item_and_then_deleting_it_shows_the_deletion() {
    for (drop_ins, name) in [(&[][..], "web"), (&[("apps.yaml", BLOG)][..], "blog")] {
        let set = Set::new(drop_ins);
        let s = set.session(true);
        let plan = set
            .plan(&s, &[rename(name, "gone"), remove("gone")])
            .unwrap();
        assert!(
            said(&plan).contains(&format!("Service {name} removed")),
            "{:?}",
            said(&plan)
        );
        let kept = set
            .plan(&s, &[rename(name, "a"), rename("a", "b")])
            .unwrap();
        assert!(
            !said(&kept).iter().any(|c| c.ends_with(" removed")),
            "{:?}",
            said(&kept)
        );
    }
}

#[test]
fn a_new_item_under_a_name_a_swap_freed_goes_to_the_main_file() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let ops = [
        rename("blog", "tmp"),
        rename("web", "blog"),
        rename("tmp", "web"),
        Op::Add {
            path: p(&["services", "tmp"]),
            value: json!({"client": "home", "bind": 7845, "target": 3005}),
        },
    ];
    set.apply(&s, &ops).unwrap();
    assert_eq!(
        set.read("server.d/apps.yaml").unwrap(),
        BLOG.replace("  blog:", "  web:")
    );
    let main = set.read("server.yaml").unwrap();
    assert!(main.contains("  tmp:"), "{main}");
    assert!(
        main.contains("  blog: { client: home, bind: 7840, target: 3000 }\n"),
        "{main}"
    );
}
