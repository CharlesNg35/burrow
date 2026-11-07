use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub name: String,
    pub base: bool,
}

pub fn origins<'a, 'b>(
    base: impl IntoIterator<Item = &'a str>,
    renames: impl IntoIterator<Item = (&'b str, &'b str)>,
) -> BTreeMap<String, Origin> {
    let mut now: BTreeMap<String, Origin> = base
        .into_iter()
        .map(|name| {
            let origin = Origin {
                name: name.to_string(),
                base: true,
            };
            (name.to_string(), origin)
        })
        .collect();
    for (from, to) in renames {
        let origin = now.remove(from).unwrap_or_else(|| Origin {
            name: from.to_string(),
            base: false,
        });
        now.insert(to.to_string(), origin);
    }
    now
}

pub fn base_name(origins: &BTreeMap<String, Origin>, name: &str) -> Option<String> {
    origins.get(name).filter(|o| o.base).map(|o| o.name.clone())
}

pub fn arrivals<'a>(
    renames: impl DoubleEndedIterator<Item = (&'a [String], &'a str)> + ExactSizeIterator,
    item: &[String],
) -> Vec<usize> {
    let Some((name, parent)) = item.split_last() else {
        return Vec::new();
    };
    let mut name = name.as_str();
    let mut out = Vec::new();
    for (i, (path, to)) in renames.enumerate().rev() {
        let Some((from, at)) = path.split_last() else {
            continue;
        };
        if at != parent {
            continue;
        }
        if to == name {
            out.push(i);
            name = from;
        } else if from == name {
            break;
        }
    }
    out
}

pub fn within<'a>(
    section: &'a str,
    renames: impl IntoIterator<Item = (&'a [String], &'a str)>,
) -> impl Iterator<Item = (&'a str, &'a str)> {
    renames
        .into_iter()
        .filter_map(move |(path, to)| match path {
            [s, from] if s == section => Some((from.as_str(), to)),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(base: &[&str], renames: &[(&str, &str)]) -> BTreeMap<String, Origin> {
        origins(base.iter().copied(), renames.iter().copied())
    }

    fn base_of(base: &[&str], renames: &[(&str, &str)], name: &str) -> Option<String> {
        base_name(&of(base, renames), name)
    }

    #[test]
    fn each_current_name_leads_back_to_its_base_name() {
        let chain = [("blog", "a"), ("a", "b")];
        assert_eq!(base_of(&["blog"], &chain, "b").as_deref(), Some("blog"));
        assert_eq!(base_of(&["blog"], &chain, "a"), None);
        assert_eq!(base_of(&["blog"], &chain, "blog"), None);
        let back = [("blog", "journal"), ("journal", "blog")];
        assert_eq!(base_of(&["blog"], &back, "blog").as_deref(), Some("blog"));
        assert_eq!(base_of(&["blog"], &back, "journal"), None);
        let swap = [("a", "t"), ("b", "a"), ("t", "b")];
        assert_eq!(base_of(&["a", "b"], &swap, "a").as_deref(), Some("b"));
        assert_eq!(base_of(&["a", "b"], &swap, "b").as_deref(), Some("a"));
        assert_eq!(base_of(&["a", "b"], &swap, "t"), None);
        assert_eq!(base_of(&["web"], &[], "web").as_deref(), Some("web"));
        assert_eq!(base_of(&["web"], &[], "new"), None);
    }

    #[test]
    fn a_name_freed_by_a_rename_and_used_again_is_new() {
        let freed = [("eta2", "eps1"), ("web", "eta2"), ("eps1", "web")];
        assert_eq!(base_of(&["eta2", "web"], &freed, "eps1"), None);
        let taken = [("old", "gone"), ("new", "old")];
        assert_eq!(
            base_of(&["old", "new"], &taken, "old").as_deref(),
            Some("new")
        );
        assert_eq!(
            base_of(&["old", "new"], &taken, "gone").as_deref(),
            Some("old")
        );
        let renamed_new = [("blog", "journal"), ("blog", "x")];
        let names = of(&["blog"], &renamed_new);
        assert_eq!(base_name(&names, "x"), None);
        assert_eq!(
            names.get("x"),
            Some(&Origin {
                name: "blog".into(),
                base: false
            })
        );
    }

    #[test]
    fn the_renames_that_brought_an_item_to_its_name_are_found_back_to_its_first_name() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let of = |renames: &[(&[&str], &str)], item: &[&str]| {
            let owned: Vec<(Vec<String>, &str)> = renames.iter().map(|(f, t)| (p(f), *t)).collect();
            arrivals(owned.iter().map(|(f, t)| (f.as_slice(), *t)), &p(item))
        };
        let chain: [(&[&str], &str); 3] = [
            (&["services", "blog"], "a"),
            (&["clients", "a"], "z"),
            (&["services", "a"], "b"),
        ];
        assert_eq!(of(&chain, &["services", "b"]), [2, 0]);
        assert_eq!(of(&chain, &["clients", "z"]), [1]);
        assert!(of(&chain, &["services", "a"]).is_empty());
        let swap: [(&[&str], &str); 3] = [
            (&["services", "a"], "t"),
            (&["services", "b"], "a"),
            (&["services", "t"], "b"),
        ];
        assert_eq!(of(&swap, &["services", "b"]), [2, 0]);
        assert_eq!(of(&swap, &["services", "a"]), [1]);
        let freed: [(&[&str], &str); 2] = [(&["services", "x"], "y"), (&["services", "y"], "z")];
        assert!(of(&freed, &["services", "y"]).is_empty());
        assert_eq!(of(&freed, &["services", "z"]), [1, 0]);
        let login: [(&[&str], &str); 1] = [(&["web", "forward_auth", "sso"], "auth")];
        assert_eq!(of(&login, &["web", "forward_auth", "auth"]), [0]);
        assert!(of(&login, &["services", "auth"]).is_empty());
    }

    #[test]
    fn only_renames_of_items_in_the_section_count() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let renames = [
            (p(&["services", "a"]), "b".to_string()),
            (p(&["clients", "x"]), "y".to_string()),
            (p(&["web", "forward_auth", "sso"]), "z".to_string()),
        ];
        let got: Vec<(&str, &str)> = within(
            "services",
            renames.iter().map(|(p, t)| (p.as_slice(), t.as_str())),
        )
        .collect();
        assert_eq!(got, [("a", "b")]);
    }
}
