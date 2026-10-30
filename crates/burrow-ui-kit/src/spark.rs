use leptos::prelude::*;

fn clean(v: f64) -> f64 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

pub fn spark_max(series: &[&[f64]]) -> f64 {
    series
        .iter()
        .flat_map(|s| s.iter().copied())
        .map(clean)
        .fold(0.0_f64, f64::max)
}

pub fn spark_path(points: &[f64], max: f64, width: f64, height: f64) -> String {
    let opt: Vec<Option<f64>> = points.iter().map(|v| Some(*v)).collect();
    segments(&opt, points.len(), max, width, height)
        .into_iter()
        .map(|seg| line_of(&seg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn segments(
    points: &[Option<f64>],
    slots: usize,
    max: f64,
    width: f64,
    height: f64,
) -> Vec<Vec<(f64, f64)>> {
    let slots = slots.max(points.len()).max(2);
    let step = width / (slots - 1) as f64;
    let offset = slots - points.len();
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        match p {
            Some(v) => {
                let y = if max > 0.0 {
                    height - (clean(*v).min(max) / max) * height
                } else {
                    height
                };
                cur.push((step * (offset + i) as f64, y));
            }
            None => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn line_of(seg: &[(f64, f64)]) -> String {
    seg.iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{}{x:.1},{y:.1}", if i == 0 { "M" } else { " L" }))
        .collect::<String>()
}

fn area_of(seg: &[(f64, f64)], height: f64) -> String {
    let (Some(first), Some(last)) = (seg.first(), seg.last()) else {
        return String::new();
    };
    format!(
        "{} L{:.1},{height:.1} L{:.1},{height:.1} Z",
        line_of(seg),
        last.0,
        first.0
    )
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartData {
    pub a: Vec<Option<f64>>,
    pub b: Vec<Option<f64>>,
    pub slots: usize,
    pub max: f64,
    pub y_ticks: Vec<String>,
    pub x_ticks: Vec<String>,
    pub empty_note: Option<String>,
}

#[component]
pub fn LineChart(
    #[prop(into)] data: Signal<ChartData>,
    #[prop(into)] label: Signal<String>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let (w, h) = (1000.0, 300.0);
    view! {
        <div class="relative flex min-h-[220px] flex-1 flex-col" data-testid=(!testid.is_empty()).then_some(testid)>
            <div class="relative flex min-h-0 flex-1">
                <div class="flex w-10 flex-col justify-between pr-2 pb-0 text-right font-mono text-2xs text-text-faint">
                    {move || data.get().y_ticks.into_iter().rev().map(|t| view! { <span class="leading-none">{t}</span> }).collect_view()}
                </div>
                <svg class="h-full min-h-[200px] w-full flex-1" viewBox=format!("0 0 {w} {h}") preserveAspectRatio="none" role="img" aria-label=move || label.get()>
                    {move || {
                        let d = data.get();
                        let n = d.y_ticks.len().max(2);
                        (0..n)
                            .map(|i| {
                                let y = h - h * i as f64 / (n - 1) as f64;
                                view! { <line x1="0" x2=w y1=y y2=y class="stroke-rowline [vector-effect:non-scaling-stroke]" stroke-width="1"></line> }
                            })
                            .collect_view()
                    }}
                    {move || {
                        let d = data.get();
                        let a = segments(&d.a, d.slots, d.max, w, h);
                        let b = segments(&d.b, d.slots, d.max, w, h);
                        let start = a.first().and_then(|s| s.first()).map(|p| p.0);
                        let ends = |segs: &Vec<Vec<(f64, f64)>>| segs.last().and_then(|s| s.last()).copied();
                        let (ea, eb) = (ends(&a), ends(&b));
                        view! {
                            {start.filter(|x| *x > 1.0).map(|x| view! { <line x1=x x2=x y1="0" y2=h class="stroke-border-strong [vector-effect:non-scaling-stroke]" stroke-width="1" stroke-dasharray="4 4"></line> })}
                            {a.iter().map(|s| view! { <path d=area_of(s, h) class="fill-accent/12 stroke-none"></path> }).collect_view()}
                            {a.iter().map(|s| view! { <path d=line_of(s) class="fill-none stroke-accent [vector-effect:non-scaling-stroke]" stroke-width="2"></path> }).collect_view()}
                            {b.iter().map(|s| view! { <path d=line_of(s) class="fill-none stroke-udp [vector-effect:non-scaling-stroke]" stroke-width="2"></path> }).collect_view()}
                            {ea.map(|(x, y)| view! { <circle cx=x cy=y r="4" class="fill-accent stroke-surface [vector-effect:non-scaling-stroke]" stroke-width="2"></circle> })}
                            {eb.map(|(x, y)| view! { <circle cx=x cy=y r="4" class="fill-udp stroke-surface [vector-effect:non-scaling-stroke]" stroke-width="2"></circle> })}
                        }
                    }}
                </svg>
                {move || data.get().empty_note.map(|t| view! {
                    <div class="pointer-events-none absolute inset-0 flex items-center justify-center pl-10 text-sm text-text-muted" data-testid="chart-note">{t}</div>
                })}
            </div>
            <div class="flex justify-between pt-1.5 pl-10 font-mono text-2xs text-text-faint">
                {move || data.get().x_ticks.into_iter().map(|t| view! { <span>{t}</span> }).collect_view()}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_scale_to_the_box() {
        assert_eq!(spark_path(&[], 1.0, 100.0, 10.0), "");
        assert_eq!(
            spark_path(&[0.0, 5.0, 10.0], 10.0, 100.0, 10.0),
            "M0.0,10.0 L50.0,5.0 L100.0,0.0"
        );
        assert_eq!(spark_max(&[&[f64::NAN, 2.0], &[-1.0, 4.0]]), 4.0);
    }

    #[test]
    fn gaps_split_the_line_and_short_data_ends_at_now() {
        let s = segments(
            &[Some(1.0), None, Some(1.0), Some(0.0)],
            5,
            1.0,
            100.0,
            10.0,
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], vec![(25.0, 0.0)]);
        assert_eq!(s[1], vec![(75.0, 0.0), (100.0, 10.0)]);
        assert!(area_of(&s[1], 10.0).ends_with("Z"));
    }
}
