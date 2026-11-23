use std::collections::HashMap;

use super::bundle::{Kind, Row};
use super::layout::{Node, Path, row_y};
use super::links::curve;
use super::{PITCH, TOP};

const MARGIN: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Spot {
    pub(super) y: f64,
    pub(super) stick: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stick {
    pub top: f64,
    pub bottom: f64,
    pub h: f64,
    pub x1: f64,
    pub x2: f64,
    pub end: Option<f64>,
}

pub(super) fn sticky_y(top: f64, bottom: f64, h: f64, view_top: f64) -> f64 {
    (view_top + MARGIN).clamp(top, (bottom - h).max(top))
}

impl Stick {
    pub(super) fn of(node: &Node, x1: f64, x2: f64, end: Option<f64>) -> Option<Stick> {
        node.stick.map(|bottom| Stick {
            top: node.y,
            bottom,
            h: node.h,
            x1,
            x2,
            end,
        })
    }

    pub(super) fn reach(&self) -> Path {
        let (top, bottom) = match self.end {
            Some(y) => (y, y),
            None => (self.top, self.bottom),
        };
        Path {
            top,
            bottom,
            ..Path::default()
        }
    }

    pub(super) fn d(&self, view_top: f64) -> String {
        let y = sticky_y(self.top, self.bottom, self.h, view_top) + self.h / 2.0;
        curve(self.x1, y, self.x2, self.end.unwrap_or(y)).d
    }
}

fn owner(r: &Row) -> Option<&str> {
    match &r.kind {
        Kind::One => r.svc.clients.first().map(String::as_str),
        Kind::Bundle(g) | Kind::Header(g) => Some(g.client.as_str()),
    }
}

fn spread(count: usize, rows: usize) -> Vec<Spot> {
    let slot = (rows as f64 * PITCH) / count.max(1) as f64;
    (0..count)
        .map(|j| Spot {
            y: TOP + (j as f64 + 0.5) * slot - 21.0,
            stick: None,
        })
        .collect()
}

pub(super) fn spots(names: &[String], rows: &[Row], height_rows: usize, h: f64) -> Vec<Spot> {
    let blocks: HashMap<&str, (usize, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match &r.kind {
            Kind::Header(g) => Some((g.client.as_str(), (i, i + g.count))),
            _ => None,
        })
        .collect();
    if blocks.is_empty() {
        return spread(names.len(), height_rows);
    }
    let mut first: HashMap<&str, usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if let Some(c) = owner(r) {
            first.entry(c).or_insert(i);
        }
    }
    for (c, (i, _)) in &blocks {
        first.insert(c, *i);
    }
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by_key(|j| first.get(names[*j].as_str()).copied().unwrap_or(usize::MAX));
    let mut out = vec![
        Spot {
            y: 0.0,
            stick: None
        };
        names.len()
    ];
    let mut next = row_y(0);
    for j in order {
        let c = names[j].as_str();
        let y = first.get(c).map_or(next, |i| row_y(*i).max(next));
        let stick = blocks.get(c).map(|(_, last)| (row_y(*last) + h).max(y + h));
        out[j] = Spot { y, stick };
        next = stick.unwrap_or(y + h) + PITCH - h;
    }
    out
}
