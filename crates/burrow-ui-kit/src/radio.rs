use leptos::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub struct RadioOption {
    pub value: String,
    pub label: String,
    pub help: String,
    pub disabled: Option<String>,
}

#[component]
pub fn RadioList(
    #[prop(into)] name: String,
    #[prop(into)] options: Signal<Vec<RadioOption>>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(optional, into)] label: String,
) -> impl IntoView {
    let rows = move || {
        options
            .get()
            .into_iter()
            .map(|o| {
                let (v1, v2, v3, v4) = (o.value.clone(), o.value.clone(), o.value.clone(), o.value.clone());
                let off = o.disabled.is_some();
                let why = o.disabled.clone();
                let text = why.clone().unwrap_or(o.help);
                let name = name.clone();
                view! {
                    <label
                        class=move || format!(
                            "flex items-start gap-2.5 rounded-input border px-3 py-2 sm:items-center {}",
                            match (value.get() == v1, off) {
                                (_, true) => "cursor-not-allowed border-border opacity-55",
                                (true, false) => "cursor-pointer border-accent-line bg-accent-weak",
                                (false, false) => "cursor-pointer border-border hover:border-border-strong",
                            }
                        )
                        title=why.clone()
                        data-option=v3
                        data-off=off.then_some("true")
                    >
                        <input
                            type="radio"
                            class="mt-0.5 size-4 shrink-0 accent-accent sm:mt-0"
                            name=name
                            value=v4
                            disabled=off
                            prop:checked=move || value.get() == v2
                            on:change=move |_| on_change.run(o.value.clone())
                        />
                        <span class="flex min-w-0 flex-1 flex-col gap-x-3 sm:flex-row sm:items-baseline">
                            <span class="shrink-0 text-sm font-semibold sm:w-56">{o.label}</span>
                            <span class="text-xs text-text-muted">{text}</span>
                        </span>
                    </label>
                }
            })
            .collect_view()
    };
    view! {
        <div class="flex flex-col gap-1.5" role="radiogroup" aria-label=(!label.is_empty()).then_some(label)>
            {rows}
        </div>
    }
}
