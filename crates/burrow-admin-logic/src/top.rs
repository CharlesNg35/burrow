use std::cmp::Reverse;

pub const ROWS: usize = 8;
pub const NAMES: usize = 3;

pub fn top<T>(mut rows: Vec<T>, rank: impl Fn(&T) -> (bool, u64)) -> (Vec<T>, usize) {
    let total = rows.len();
    rows.sort_by_cached_key(|r| {
        let (problem, busy) = rank(r);
        (Reverse(problem), Reverse(busy))
    });
    rows.truncate(ROWS);
    (rows, total)
}

pub fn view_all(shown: usize, total: usize) -> Option<String> {
    (total > shown).then(|| format!("View all {total} →"))
}

pub fn some_names(all: &[String]) -> (String, Option<String>) {
    let shown = all
        .iter()
        .take(NAMES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = all.len().saturating_sub(NAMES);
    (shown, (more > 0).then(|| format!("+{more} more")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_come_first_then_the_busiest_and_at_most_8_rows_show() {
        let rows: Vec<(String, bool, u64)> = (0..50)
            .map(|i| (format!("c{i:02}"), i == 40 || i == 45, i as u64 % 7))
            .collect();
        let (kept, total) = top(rows, |r| (r.1, r.2));
        assert_eq!(total, 50);
        assert_eq!(kept.len(), ROWS);
        let names: Vec<&str> = kept.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(&names[..2], ["c40", "c45"]);
        assert!(kept[2..].iter().all(|r| r.2 == 6));
        assert_eq!(&names[2..4], ["c06", "c13"], "ties keep the name order");
    }

    #[test]
    fn view_all_shows_only_when_rows_are_left_out() {
        assert_eq!(view_all(8, 50).as_deref(), Some("View all 50 →"));
        assert_eq!(view_all(8, 8), None);
        assert_eq!(view_all(3, 3), None);
    }

    #[test]
    fn a_long_list_of_names_shows_three_then_how_many_more() {
        let all: Vec<String> = (1..=30).map(|i| format!("s{i}")).collect();
        assert_eq!(
            some_names(&all),
            ("s1, s2, s3".to_string(), Some("+27 more".to_string()))
        );
        let few: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(some_names(&few), ("a, b".to_string(), None));
        assert_eq!(some_names(&[]), (String::new(), None));
    }
}
