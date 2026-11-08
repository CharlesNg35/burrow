use serde::{Deserialize, Serialize};

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;
pub const EXCERPT_CHARS: usize = 120;
const CUT: char = '…';

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchText {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl SearchText {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchReply {
    pub rev: String,
    #[serde(default)]
    pub matches: Vec<TextMatch>,
    #[serde(default)]
    pub more: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsearchable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMatch {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub end_col: u32,
    pub excerpt: String,
}

fn folded(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn excerpt(line: &[char], at: usize, len: usize) -> String {
    if line.len() <= EXCERPT_CHARS {
        return line.iter().collect::<String>().trim().to_string();
    }
    let room = EXCERPT_CHARS.saturating_sub(len) / 2;
    let start = at
        .saturating_sub(room)
        .min(line.len().saturating_sub(EXCERPT_CHARS));
    let end = (start + EXCERPT_CHARS.max(len)).min(line.len());
    let mut out: String = line[start..end].iter().collect();
    out = out.trim().to_string();
    if start > 0 {
        out.insert(0, CUT);
    }
    if end < line.len() {
        out.push(CUT);
    }
    out
}

pub fn find(files: &[(&str, &str)], query: &str, limit: usize) -> (Vec<TextMatch>, bool) {
    let query: Vec<char> = query.trim().chars().map(folded).collect();
    let mut out = Vec::new();
    if query.is_empty() {
        return (out, false);
    }
    for (file, text) in files {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (i, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let chars: Vec<char> = line.chars().collect();
            let lower: Vec<char> = chars.iter().copied().map(folded).collect();
            let mut at = 0;
            while at + query.len() <= lower.len() {
                if lower[at..at + query.len()] != query[..] {
                    at += 1;
                    continue;
                }
                if out.len() == limit {
                    return (out, true);
                }
                out.push(TextMatch {
                    file: file.to_string(),
                    line: i as u32 + 1,
                    col: at as u32 + 1,
                    end_col: (at + query.len()) as u32 + 1,
                    excerpt: excerpt(&chars, at, query.len()),
                });
                at += query.len();
            }
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_case_blind_per_line_with_one_based_columns() {
        let (found, more) = find(
            &[
                (
                    "server.yaml",
                    "\u{feff}listen: 7835\r\nservices:\r\n  Web: {}\r\n",
                ),
                ("server.d/a.yaml", "# web ÉTÉ été\n"),
            ],
            "WEB",
            10,
        );
        assert!(!more);
        let at: Vec<(&str, u32, u32, u32)> = found
            .iter()
            .map(|m| (m.file.as_str(), m.line, m.col, m.end_col))
            .collect();
        assert_eq!(at, [("server.yaml", 3, 3, 6), ("server.d/a.yaml", 1, 3, 6)]);
        assert_eq!(found[0].excerpt, "Web: {}");
        let (ete, _) = find(&[("a", "# web ÉTÉ été\n")], "été", 10);
        assert_eq!(ete.iter().map(|m| m.col).collect::<Vec<_>>(), [7, 11]);
    }

    #[test]
    fn a_blank_query_finds_nothing_and_the_limit_says_there_is_more() {
        assert!(find(&[("a", "x\n")], "  ", 10).0.is_empty());
        let text = "aaaa\n".repeat(10);
        let (found, more) = find(&[("a", &text)], "a", 3);
        assert_eq!((found.len(), more), (3, true));
        let (all, more) = find(&[("a", "a\n")], "a", 1);
        assert_eq!((all.len(), more), (1, false));
        assert_eq!(
            SearchText {
                query: "x".into(),
                limit: Some(10_000)
            }
            .limit(),
            MAX_LIMIT as usize
        );
        assert_eq!(
            SearchText {
                query: "x".into(),
                limit: None
            }
            .limit(),
            DEFAULT_LIMIT as usize
        );
    }

    #[test]
    fn a_long_line_is_cut_around_the_match() {
        let line = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        let (found, _) = find(&[("a", &line)], "needle", 5);
        let e = &found[0].excerpt;
        assert!(
            e.contains("needle") && e.starts_with(CUT) && e.ends_with(CUT),
            "{e}"
        );
        assert!(e.chars().count() <= EXCERPT_CHARS + 2, "{e}");
    }
}
