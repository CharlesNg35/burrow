use std::fmt::Write;

use super::{Paint, Tone};

#[derive(Clone)]
pub(super) struct Cell {
    pub(super) parts: Vec<(String, Tone)>,
    pub(super) right: bool,
}

impl Cell {
    pub(super) fn clip(&self, width: usize, tail: bool) -> Cell {
        if self.width() <= width {
            return self.clone();
        }
        if tail {
            return self.clip_start(width);
        }
        let mut left = width.saturating_sub(1);
        let mut parts = Vec::new();
        let mut last = Tone::Plain;
        for (text, tone) in &self.parts {
            if left == 0 {
                break;
            }
            let taken: String = text.chars().take(left).collect();
            left -= taken.chars().count();
            last = *tone;
            parts.push((taken, *tone));
        }
        while let Some((text, _)) = parts.last_mut() {
            let trimmed = text.trim_end().len();
            text.truncate(trimmed);
            if !text.is_empty() {
                break;
            }
            parts.pop();
        }
        parts.push(("…".into(), last));
        Cell {
            parts,
            right: self.right,
        }
    }

    fn clip_start(&self, width: usize) -> Cell {
        let mut left = width.saturating_sub(1);
        let mut parts = Vec::new();
        let mut first = Tone::Plain;
        for (text, tone) in self.parts.iter().rev() {
            if left == 0 {
                break;
            }
            let count = text.chars().count();
            let taken: String = text.chars().skip(count.saturating_sub(left)).collect();
            left -= taken.chars().count();
            first = *tone;
            parts.push((taken, *tone));
        }
        parts.reverse();
        while let Some((text, _)) = parts.first_mut() {
            *text = text.trim_start().to_string();
            if !text.is_empty() {
                break;
            }
            parts.remove(0);
        }
        parts.insert(0, ("…".into(), first));
        Cell {
            parts,
            right: self.right,
        }
    }

    pub(super) fn width(&self) -> usize {
        self.parts.iter().map(|(t, _)| t.chars().count()).sum()
    }

    pub(super) fn painted(&self, paint: Paint) -> String {
        self.parts
            .iter()
            .map(|(t, tone)| paint.tone(t, *tone))
            .collect()
    }
}

pub(super) fn toned(text: impl Into<String>, tone: Tone) -> Cell {
    Cell {
        parts: vec![(text.into(), tone)],
        right: false,
    }
}

pub(super) fn cell(text: impl Into<String>) -> Cell {
    toned(text, Tone::Plain)
}

pub(super) fn num(n: u64) -> Cell {
    Cell {
        right: true,
        ..toned(n.to_string(), if n == 0 { Tone::Dim } else { Tone::Plain })
    }
}

pub(super) fn right(text: impl Into<String>, tone: Tone) -> Cell {
    Cell {
        right: true,
        ..toned(text, tone)
    }
}

pub(super) const ALWAYS: u8 = u8::MAX;

const CAP: usize = 24;

const FLOOR: usize = 10;

const CHEAP: u8 = 3;

pub(super) struct Col {
    name: &'static str,
    keep: u8,
    long: bool,
    tail: bool,
}

pub(super) fn col(name: &'static str, keep: u8) -> Col {
    Col {
        name,
        keep,
        long: false,
        tail: false,
    }
}

pub(super) fn long(name: &'static str, keep: u8) -> Col {
    Col {
        long: true,
        ..col(name, keep)
    }
}

pub(super) fn address(name: &'static str, keep: u8) -> Col {
    Col {
        tail: true,
        ..long(name, keep)
    }
}

fn span(widths: &[usize], shown: &[bool], boxed: bool) -> usize {
    let visible: Vec<usize> = widths
        .iter()
        .zip(shown)
        .filter(|(_, s)| **s)
        .map(|(w, _)| *w)
        .collect();
    if boxed {
        1 + visible.iter().map(|w| w + 3).sum::<usize>()
    } else {
        2 + visible.iter().sum::<usize>() + 2 * visible.len().saturating_sub(1)
    }
}

fn fit(cols: &[Col], widths: &mut [usize], shown: &mut [bool], max: usize, boxed: bool) {
    if span(widths, shown, boxed) <= max {
        return;
    }
    let floor = |i: usize| FLOOR.max(cols[i].name.chars().count());
    for (i, c) in cols.iter().enumerate() {
        if c.long {
            widths[i] = widths[i].min(CAP.max(floor(i)));
        }
    }
    loop {
        let need = span(widths, shown, boxed).saturating_sub(max);
        if need == 0 {
            return;
        }
        let room: usize = (0..cols.len())
            .filter(|&i| shown[i] && cols[i].long && !cols[i].tail)
            .map(|i| widths[i].saturating_sub(floor(i)))
            .sum();
        let weakest = (0..cols.len())
            .filter(|&i| shown[i] && cols[i].keep < ALWAYS)
            .min_by_key(|&i| (cols[i].keep, std::cmp::Reverse(i)));
        match weakest {
            Some(i) if cols[i].keep <= CHEAP || room < need => {
                let keep = cols[i].keep;
                for (j, c) in cols.iter().enumerate() {
                    if c.keep == keep {
                        shown[j] = false;
                    }
                }
            }
            _ => {
                let any = weakest.is_none();
                for _ in 0..need {
                    let widest = (0..cols.len())
                        .filter(|&i| {
                            shown[i]
                                && cols[i].long
                                && (any || !cols[i].tail)
                                && widths[i] > floor(i)
                        })
                        .max_by_key(|&i| widths[i] - floor(i));
                    match widest {
                        Some(i) => widths[i] -= 1,
                        None => return,
                    }
                }
                return;
            }
        }
    }
}

pub(super) fn table(
    out: &mut String,
    paint: Paint,
    title: &str,
    cols: &[Col],
    rows: Vec<Vec<Cell>>,
) {
    let _ = writeln!(out, "\n{title}");
    let mut widths: Vec<usize> = cols.iter().map(|c| c.name.chars().count()).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.width());
        }
    }
    let mut shown = vec![true; cols.len()];
    if let Some(max) = paint.width {
        fit(cols, &mut widths, &mut shown, max, paint.boxed);
    }
    let visible: Vec<usize> = (0..cols.len()).filter(|&i| shown[i]).collect();
    let aligned: Vec<bool> = (0..cols.len())
        .map(|i| rows.first().is_some_and(|r| r[i].right))
        .collect();
    let pad = |text: String, width: usize, seen: usize, right_aligned: bool| {
        let fill = " ".repeat(width.saturating_sub(seen));
        if right_aligned {
            format!("{fill}{text}")
        } else {
            format!("{text}{fill}")
        }
    };
    let head: Vec<String> = visible
        .iter()
        .map(|&i| {
            let tone = if paint.boxed { Tone::Bold } else { Tone::Dim };
            let name = cols[i].name;
            pad(
                paint.tone(name, tone),
                widths[i],
                name.chars().count(),
                aligned[i],
            )
        })
        .collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            visible
                .iter()
                .map(|&i| {
                    let c = row[i].clip(widths[i], cols[i].tail);
                    pad(c.painted(paint), widths[i], c.width(), c.right)
                })
                .collect()
        })
        .collect();
    if paint.boxed {
        let rule = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = visible.iter().map(|&i| "─".repeat(widths[i] + 2)).collect();
            paint.tone(&format!("{l}{}{r}", inner.join(m)), Tone::Dim)
        };
        let bar = paint.tone("│", Tone::Dim);
        let line = |cells: &Vec<String>| format!("{bar} {} {bar}", cells.join(&format!(" {bar} ")));
        let _ = writeln!(out, "{}", rule("╭", "┬", "╮"));
        let _ = writeln!(out, "{}", line(&head));
        let _ = writeln!(out, "{}", rule("├", "┼", "┤"));
        for cells in &body {
            let _ = writeln!(out, "{}", line(cells));
        }
        let _ = writeln!(out, "{}", rule("╰", "┴", "╯"));
    } else {
        for cells in std::iter::once(&head).chain(&body) {
            let line = format!("  {}", cells.join("  "));
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
    let hidden: Vec<&str> = (0..cols.len())
        .filter(|&i| !shown[i])
        .map(|i| cols[i].name)
        .collect();
    if !hidden.is_empty() {
        let note = paint.clip(&format!("  hidden to fit: {}", hidden.join(", ")));
        let _ = writeln!(out, "{}", paint.tone(&note, Tone::Dim));
    }
}
