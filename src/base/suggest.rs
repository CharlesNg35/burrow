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
        .map(|o| (strsim::osa_distance(&lower, &o.to_ascii_lowercase()), o))
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
        let d = strsim::osa_distance;
        assert_eq!(d("bnd", "bind"), 1);
        assert_eq!(d("kitten", "sitting"), 3);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("tpc", "tcp"), 1);
    }

    #[test]
    fn a_huge_unknown_key_is_compared_quickly() {
        let huge = "x".repeat(200_000);
        let started = std::time::Instant::now();
        assert_eq!(closest(&huge, ["bind", "client", "target"]), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
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
