#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Viewport {
    pub top: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub from: f64,
    pub to: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Snap {
    Exact,
    Step(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub above: f64,
    pub below: f64,
}

impl Viewport {
    pub fn band(self, around: f64, snap: Snap) -> Band {
        let (from, to) = (self.top - around, self.top + self.height + around);
        match snap {
            Snap::Step(step) if step > 0.0 => Band {
                from: (from / step).floor() * step,
                to: (to / step).ceil() * step,
            },
            _ => Band { from, to },
        }
    }
}

pub fn span(sizes: &[f64], band: Band) -> Span {
    let mut at = 0.0;
    let mut start = None;
    let mut above = 0.0;
    let mut end = sizes.len();
    for (i, h) in sizes.iter().enumerate() {
        if start.is_none() && at + h > band.from {
            start = Some(i);
            above = at;
        }
        if at >= band.to {
            end = i;
            break;
        }
        at += h;
    }
    let start = start.unwrap_or(sizes.len()).min(end);
    let total: f64 = sizes.iter().sum();
    let shown: f64 = sizes[start..end].iter().sum();
    if start == sizes.len() {
        above = total;
    }
    Span {
        start,
        end,
        above,
        below: (total - above - shown).max(0.0),
    }
}

pub fn offset(sizes: &[f64], index: usize) -> f64 {
    sizes.iter().take(index).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(top: f64, height: f64) -> Viewport {
        Viewport { top, height }
    }

    #[test]
    fn the_span_holds_the_items_in_view_and_a_margin_around_them() {
        let h = vec![100.0; 1000];
        let s = span(&h, view(50_000.0, 900.0).band(800.0, Snap::Exact));
        assert_eq!((s.start, s.end, s.above), (492, 517, 49_200.0));
        assert_eq!(s.above + 2500.0 + s.below, 100_000.0);
        let top = span(&h, view(0.0, 900.0).band(800.0, Snap::Exact));
        assert_eq!((top.start, top.above), (0, 0.0));
        let few = span(&[50.0; 3], view(0.0, 900.0).band(800.0, Snap::Exact));
        assert_eq!((few.start, few.end, few.below), (0, 3, 0.0));
        assert_eq!(
            span(&[], view(0.0, 900.0).band(0.0, Snap::Exact)),
            Span::default()
        );
    }

    #[test]
    fn measured_sizes_of_any_height_add_up() {
        let h = [40.0, 120.0, 80.0, 300.0, 60.0];
        let s = span(
            &h,
            Band {
                from: 150.0,
                to: 250.0,
            },
        );
        assert_eq!((s.start, s.end), (1, 4));
        assert_eq!((s.above, s.below), (40.0, 60.0));
        assert_eq!(offset(&h, 3), 240.0);
        let past = span(
            &h,
            Band {
                from: 5000.0,
                to: 6000.0,
            },
        );
        assert_eq!(
            (past.start, past.end, past.above, past.below),
            (5, 5, 600.0, 0.0)
        );
    }

    #[test]
    fn a_stepped_band_moves_only_in_whole_steps() {
        let b = view(1000.0, 600.0).band(600.0, Snap::Step(400.0));
        assert_eq!(
            b,
            Band {
                from: 400.0,
                to: 2400.0
            }
        );
        assert_eq!(view(1010.0, 600.0).band(600.0, Snap::Step(400.0)), b);
        assert_eq!(
            view(10.0, 600.0).band(600.0, Snap::Exact),
            Band {
                from: -590.0,
                to: 1210.0
            }
        );
    }
}
