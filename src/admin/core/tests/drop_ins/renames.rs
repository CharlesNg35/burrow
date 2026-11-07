#![cfg(test)]

use burrow_admin_proto::ops::Op;
use serde_json::json;

use super::super::p;
use super::{BLOG, Set, rename};

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
