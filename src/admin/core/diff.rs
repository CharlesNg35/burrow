use burrow_admin_proto::plan::{Diff, Hunk};

const CONTEXT: usize = 3;
const MAX_EDITS: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Same,
    Del,
    Add,
}

fn script(a: &[&str], b: &[&str]) -> Vec<Step> {
    search(a, b).0
}

/// The edit script, and how many trace entries the search kept.
fn search(a: &[&str], b: &[&str]) -> (Vec<Step>, usize) {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let limit = max.min(MAX_EDITS);
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // Round d only reads diagonals -(d+1), -(d-1), ..., d+1 when it walks
    // back, so only those d+2 values are kept: the trace grows with the
    // number of edits, never with the length of the files.
    let mut trace: Vec<Vec<u32>> = Vec::new();
    let mut found = false;
    'outer: for d in 0..=limit as isize {
        trace.push(
            (-(d + 1)..=d + 1)
                .step_by(2)
                .map(|k| v[(k + off) as usize] as u32)
                .collect(),
        );
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = true;
                break 'outer;
            }
            k += 2;
        }
    }
    if !found {
        let mut out = vec![Step::Del; a.len()];
        out.extend(std::iter::repeat_n(Step::Add, b.len()));
        return (out, trace.iter().map(Vec::len).sum());
    }
    let mut steps = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let band = &trace[d as usize];
        let at = |k: isize| band[((k + d + 1) / 2) as usize] as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            steps.push(Step::Same);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                steps.push(Step::Add);
            } else {
                steps.push(Step::Del);
            }
        }
        x = prev_x;
        y = prev_y;
    }
    steps.reverse();
    (steps, trace.iter().map(Vec::len).sum())
}

pub fn unified(old: &str, new: &str, mask: &dyn Fn(&str) -> String) -> Diff {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let steps = script(&a, &b);
    let mut rows: Vec<(Step, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    for s in steps {
        rows.push((s, i, j));
        match s {
            Step::Same => {
                i += 1;
                j += 1;
            }
            Step::Del => i += 1,
            Step::Add => j += 1,
        }
    }
    let changed: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.0 != Step::Same)
        .map(|(k, _)| k)
        .collect();
    let mut hunks = Vec::new();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(CONTEXT);
        let mut end = changed[k];
        while k + 1 < changed.len() && changed[k + 1] <= end + 2 * CONTEXT + 1 {
            k += 1;
            end = changed[k];
        }
        let end = (end + CONTEXT + 1).min(rows.len());
        let mut lines = Vec::new();
        let (mut old_lines, mut new_lines) = (0u32, 0u32);
        for r in &rows[start..end] {
            match r.0 {
                Step::Same => {
                    lines.push((" ".to_string(), mask(a[r.1])));
                    old_lines += 1;
                    new_lines += 1;
                }
                Step::Del => {
                    lines.push(("-".to_string(), mask(a[r.1])));
                    old_lines += 1;
                }
                Step::Add => {
                    lines.push(("+".to_string(), mask(b[r.2])));
                    new_lines += 1;
                }
            }
        }
        let first = rows[start];
        hunks.push(Hunk {
            old_start: first.1 as u32 + 1,
            old_lines,
            new_start: first.2 as u32 + 1,
            new_lines,
            lines,
        });
        k += 1;
    }
    let mut text = String::new();
    for h in &hunks {
        text.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        ));
        for (tag, line) in &h.lines {
            text.push_str(tag);
            text.push_str(line);
            text.push('\n');
        }
    }
    Diff {
        unified: text,
        hunks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn small_changes_give_small_hunks() {
        let old = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old.replace("line 10\n", "line ten\n") + "line 21\n";
        let d = unified(&old, &new, &same);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[0].old_start, 7);
        assert_eq!(d.hunks[0].lines[3], ("-".into(), "line 10".into()));
        assert_eq!(d.hunks[0].lines[4], ("+".into(), "line ten".into()));
        assert_eq!(
            d.hunks[1].lines.last().unwrap(),
            &("+".into(), "line 21".into())
        );
        assert!(d.unified.starts_with("@@ -7,7 +7,7 @@\n"));
        assert!(unified(&old, &old, &same).hunks.is_empty());
        let all = unified("", "a\nb\n", &same);
        assert_eq!(all.hunks[0].new_lines, 2);
    }

    /// The search as it was before its trace was made compact, to check
    /// that the scripts stay the same.
    fn reference(a: &[&str], b: &[&str]) -> (Vec<Step>, usize) {
        let (n, m) = (a.len() as isize, b.len() as isize);
        let max = (n + m) as usize;
        let limit = max.min(MAX_EDITS);
        let off = max as isize + 1;
        let mut v = vec![0isize; 2 * max + 3];
        let mut trace: Vec<Vec<isize>> = Vec::new();
        let mut found = false;
        'outer: for d in 0..=limit as isize {
            trace.push(v.clone());
            let mut k = -d;
            while k <= d {
                let idx = (k + off) as usize;
                let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                    v[idx + 1]
                } else {
                    v[idx - 1] + 1
                };
                let mut y = x - k;
                while x < n && y < m && a[x as usize] == b[y as usize] {
                    x += 1;
                    y += 1;
                }
                v[idx] = x;
                if x >= n && y >= m {
                    found = true;
                    break 'outer;
                }
                k += 2;
            }
        }
        if !found {
            let mut out = vec![Step::Del; a.len()];
            out.extend(std::iter::repeat_n(Step::Add, b.len()));
            return (out, trace.iter().map(Vec::len).sum());
        }
        let mut steps = Vec::new();
        let (mut x, mut y) = (n, m);
        for d in (0..trace.len() as isize).rev() {
            let v = &trace[d as usize];
            let k = x - y;
            let prev_k =
                if k == -d || (k != d && v[(k - 1 + off) as usize] < v[(k + 1 + off) as usize]) {
                    k + 1
                } else {
                    k - 1
                };
            let prev_x = v[(prev_k + off) as usize];
            let prev_y = prev_x - prev_k;
            while x > prev_x && y > prev_y {
                steps.push(Step::Same);
                x -= 1;
                y -= 1;
            }
            if d > 0 {
                if x == prev_x {
                    steps.push(Step::Add);
                } else {
                    steps.push(Step::Del);
                }
            }
            x = prev_x;
            y = prev_y;
        }
        steps.reverse();
        (steps, trace.iter().map(Vec::len).sum())
    }

    fn lines(seed: &mut u64, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| {
                *seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                format!("l{}", (*seed >> 33) % 4)
            })
            .collect()
    }

    #[test]
    fn scripts_match_the_full_trace_search() {
        let mut seed = 7u64;
        for round in 0..400 {
            let (n, m) = if round < 390 {
                (round % 23, (round * 7) % 19)
            } else {
                (150 + round % 7, 140 + round % 5)
            };
            let a = lines(&mut seed, n);
            let b = lines(&mut seed, m);
            let a: Vec<&str> = a.iter().map(String::as_str).collect();
            let b: Vec<&str> = b.iter().map(String::as_str).collect();
            assert_eq!(search(&a, &b).0, reference(&a, &b).0, "{a:?} {b:?}");
        }
    }

    #[test]
    fn a_long_list_removed_keeps_a_small_trace() {
        let old: String = (0..15_000)
            .map(|i| format!("  - 10.{}.{}.0/24\n", i / 256, i % 256))
            .collect();
        let new = "  - 192.0.2.0/24\n";
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let (steps, kept) = search(&a, &b);
        assert_eq!(steps.len(), a.len() + b.len());
        assert!(kept <= MAX_EDITS * MAX_EDITS, "kept {kept} trace entries");
        let d = unified(&old, new, &same);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].old_lines, 15_000);
    }
}
