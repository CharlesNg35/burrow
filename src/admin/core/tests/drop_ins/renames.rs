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

fn place(name: &str, file: &str) -> Op {
    Op::Place {
        path: p(&["services", name]),
        file: file.into(),
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
fn a_drop_in_with_an_empty_section_no_other_file_has_is_still_edited() {
    let team = format!(
        "# team\nclients:\n  laptop:\n    secret: {}\nservices:\n",
        crate::wire::secret::Secret::generate().encode()
    );
    let wiki = json!({"client": "home", "bind": 7845, "target": 3005});
    let changes = [
        vec![Op::Remove {
            path: p(&["clients", "laptop"]),
        }],
        vec![
            Op::Add {
                path: p(&["services", "wiki"]),
                value: wiki,
            },
            place("wiki", "server.d/team.yaml"),
        ],
    ];
    for ops in changes {
        let set = Set::new(&[("team.yaml", &team)]);
        let main = set.w.text.replace(
            "\nservices:\n  web: { client: home, bind: 7840, target: 3000 }\n",
            "",
        );
        std::fs::write(set.path("server.yaml"), &main).unwrap();
        let s = set.session(true);
        set.apply(&s, &ops).unwrap();
        let after = set.read("server.d/team.yaml").unwrap();
        assert!(after.starts_with("# team\n"), "{after}");
        assert_eq!(after.contains("laptop"), ops.len() == 2, "{after}");
        assert_eq!(after.contains("wiki"), ops.len() == 2, "{after}");
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

const THREE: &str = "# apps\nservices:\n  old: # bye\n    client: home\n    bind: 7841\n    target: 3001\n  other: # untouched\n    client: home # really\n    bind: 7843\n    target: 3003\n  new: # and me\n    client: home\n    bind: 7842\n    target: 3002\n";

const OLD: &str = "  old: # bye\n    client: home\n    bind: 7841\n    target: 3001\n";

#[test]
fn renames_mixed_with_deletes_keep_the_comments_of_untouched_items() {
    let cases = [
        (
            vec![rename("old", "gone"), remove("gone")],
            THREE.replace(OLD, ""),
        ),
        (
            vec![remove("old"), rename("new", "old")],
            THREE
                .replace(OLD, "")
                .replace("  new: # and me", "  old: # and me"),
        ),
        (
            vec![rename("old", "x"), rename("x", "old"), remove("old")],
            THREE.replace(OLD, ""),
        ),
    ];
    for (ops, want) in cases {
        let set = Set::new(&[("apps.yaml", THREE)]);
        let s = set.session(true);
        let plan = set.plan(&s, &ops).unwrap();
        assert_eq!(plan.reformat, None, "{ops:?}");
        set.apply(&s, &ops).unwrap();
        assert_eq!(set.read("server.d/apps.yaml").unwrap(), want, "{ops:?}");
        assert_eq!(set.read("server.yaml").unwrap(), set.w.text, "{ops:?}");
    }
}

#[test]
fn swapping_two_names_and_moving_one_out_keeps_the_comments_of_the_other() {
    let set = Set::new(&[]);
    let main = format!(
        "{}  a: {{ client: home, bind: 7841, target: 3001 }} # a\n  b: {{ client: home, bind: 7842, target: 3002 }} # b\n",
        set.w.text
    );
    std::fs::write(set.path("server.yaml"), &main).unwrap();
    let s = set.session(true);
    let ops = [
        rename("a", "t"),
        rename("b", "a"),
        rename("t", "b"),
        place("a", "server.d/a.yaml"),
    ];
    let plan = set.plan(&s, &ops).unwrap();
    assert_eq!(plan.reformat, None);
    set.apply(&s, &ops).unwrap();
    let after = set.read("server.yaml").unwrap();
    assert_eq!(
        after,
        format!(
            "{}  b: {{ client: home, bind: 7841, target: 3001 }} # a\n",
            set.w.text
        )
    );
    let moved = set.read("server.d/a.yaml").unwrap();
    assert!(moved.contains("  a:") && moved.contains("3002"), "{moved}");
}

#[test]
fn a_place_sent_before_a_rename_or_delete_follows_the_item() {
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let gone = set
        .plan(&s, &[place("blog", "server.yaml"), remove("blog")])
        .unwrap();
    assert!(
        !said(&gone).iter().any(|c| c.starts_with("moves ")),
        "{:?}",
        said(&gone)
    );
    let ops = [place("blog", "server.yaml"), rename("blog", "journal")];
    let plan = set.plan(&s, &ops).unwrap();
    assert!(
        said(&plan)
            .contains(&"moves service journal from server.d/apps.yaml to server.yaml".into()),
        "{:?}",
        said(&plan)
    );
    set.apply(&s, &ops).unwrap();
    assert!(set.read("server.yaml").unwrap().contains("  journal:"));
    assert!(!set.read("server.d/apps.yaml").unwrap().contains("journal"));
    let set = Set::new(&[("apps.yaml", BLOG)]);
    let s = set.session(true);
    let ops = [
        place("web", "server.d/apps.yaml"),
        rename("web", "www"),
        rename("blog", "web"),
    ];
    set.apply(&s, &ops).unwrap();
    let apps = set.read("server.d/apps.yaml").unwrap();
    assert!(apps.contains("  www:") && apps.contains("  web:"), "{apps}");
    let main = set.read("server.yaml").unwrap();
    assert!(
        !main.contains("  www") && !main.contains("  web:"),
        "{main}"
    );
}

fn add(name: &str, bind: u16) -> Op {
    Op::Add {
        path: p(&["services", name]),
        value: json!({"client": "home", "bind": bind, "target": 3005}),
    }
}

#[test]
fn a_name_freed_by_a_rename_and_a_delete_holds_a_new_item_in_the_main_file() {
    for (drop_ins, name) in [(&[][..], "web"), (&[("apps.yaml", BLOG)][..], "blog")] {
        let set = Set::new(drop_ins);
        let s = set.session(true);
        let ops = [rename(name, "www"), remove("www"), add("www", 7845)];
        let plan = set.plan(&s, &ops).unwrap();
        assert_eq!(
            said(&plan),
            [
                "New service www".to_string(),
                format!("Service {name} removed")
            ],
            "{:?}",
            said(&plan)
        );
        set.apply(&s, &ops).unwrap();
        let main = set.read("server.yaml").unwrap();
        assert!(main.contains("  www:"), "{main}");
        assert!(
            !main.contains("www: {"),
            "a new item, not the old line: {main}"
        );
        if let Some(apps) = set.read("server.d/apps.yaml") {
            assert!(!apps.contains("www") && !apps.contains("blog:"), "{apps}");
        }
    }
}

#[test]
fn the_end_of_a_rename_chain_reads_as_one_rename_and_a_rename_back_as_none() {
    for (drop_ins, name) in [(&[][..], "web"), (&[("apps.yaml", BLOG)][..], "blog")] {
        let set = Set::new(drop_ins);
        let s = set.session(true);
        let chain = set
            .plan(&s, &[rename(name, "a"), rename("a", "b")])
            .unwrap();
        assert_eq!(said(&chain), [format!("Service {name} renamed to b")]);
        let back = set
            .plan(&s, &[rename(name, "a"), rename("a", name)])
            .unwrap();
        assert!(said(&back).is_empty(), "{:?}", said(&back));
    }
}
