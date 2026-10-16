pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    d[0] = (0..=b.len()).collect();
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[a.len()][b.len()]
}

pub fn closest<'a, I>(input: &str, options: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let limit = (input.chars().count() / 3).max(1);
    let lower = input.to_ascii_lowercase();
    let options: Vec<&'a str> = options.into_iter().collect();
    if lower.len() >= 2
        && let Some(p) = options
            .iter()
            .find(|o| o.to_ascii_lowercase().starts_with(&lower))
    {
        return Some(p);
    }
    options
        .into_iter()
        .map(|o| (edit_distance(&lower, &o.to_ascii_lowercase()), o))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, o)| o)
}

pub fn quoted_list<'a, I>(items: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    items.into_iter().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance() {
        assert_eq!(edit_distance("bnd", "bind"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("tpc", "tcp"), 1);
    }

    #[test]
    fn closest_respects_limit() {
        let opts = ["bind", "client", "target", "protocol"];
        assert_eq!(closest("bnd", opts), Some("bind"));
        assert_eq!(closest("protcol", opts), Some("protocol"));
        assert_eq!(closest("zzz", opts), None);
        assert_eq!(closest("hom", ["home", "office"]), Some("home"));
        assert_eq!(closest("aes", ["chachapoly", "aes-gcm"]), Some("aes-gcm"));
    }
}
