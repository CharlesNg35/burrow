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
    if points.is_empty() {
        return String::new();
    }
    let step = if points.len() > 1 {
        width / (points.len() - 1) as f64
    } else {
        0.0
    };
    let mut out = String::new();
    for (i, v) in points.iter().enumerate() {
        let y = if max > 0.0 {
            height - (clean(*v).min(max) / max) * height
        } else {
            height
        };
        out.push_str(&format!(
            "{}{:.1},{:.1}",
            if i == 0 { "M" } else { " L" },
            step * i as f64,
            y
        ));
    }
    out
}

#[component]
pub fn Sparkline(
    #[prop(into)] points: Signal<Vec<f64>>,
    #[prop(into)] second: Signal<Vec<f64>>,
    #[prop(into)] label: String,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let (w, h) = (300.0, 60.0);
    let line = move |first: bool| {
        let (a, b) = (points.get(), second.get());
        let max = spark_max(&[&a, &b]);
        spark_path(if first { &a } else { &b }, max, w, h)
    };
    view! {
        <svg
            class="h-16 w-full"
            viewBox=format!("0 0 {w} {h}")
            preserveAspectRatio="none"
            role="img"
            aria-label=label
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <path class="fill-none stroke-accent stroke-2 [vector-effect:non-scaling-stroke]" d=move || line(true)></path>
            <path class="fill-none stroke-success stroke-2 [vector-effect:non-scaling-stroke]" d=move || line(false)></path>
        </svg>
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
        assert_eq!(
            spark_path(&[0.0, 0.0], 0.0, 100.0, 10.0),
            "M0.0,10.0 L100.0,10.0"
        );
        assert_eq!(spark_max(&[&[f64::NAN, 2.0], &[-1.0, 4.0]]), 4.0);
        assert_eq!(
            spark_path(&[f64::NAN, 2.0], 4.0, 10.0, 10.0),
            "M0.0,10.0 L10.0,5.0"
        );
    }
}
