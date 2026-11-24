//! The flow chart of the admin UI: sources, public ports, the server, clients and targets in columns, colored by health and shown in `FlowModal`.

mod bundle;
mod draw;
mod layout;
mod links;
mod live;
mod place;
mod view;

pub(crate) use layout::{Act, Chart, Focus, Node, layout};
pub(crate) use view::FlowModal;

pub(crate) const SEARCH: &str = "flow-search-box";

pub const PITCH: f64 = 54.0;
pub const TOP: f64 = 26.0;
pub const WIDTH: f64 = 1080.0;
const COLS: [(f64, f64); 5] = [
    (0.0, 180.0),
    (220.0, 190.0),
    (450.0, 140.0),
    (630.0, 190.0),
    (860.0, 220.0),
];
const LABELS: [&str; 5] = ["Sources", "Public ports", "Server", "Clients", "Targets"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Health {
    Healthy,
    Degraded,
    Down,
    Offline,
    #[default]
    Unknown,
}

impl Health {
    fn stroke(self) -> &'static str {
        match self {
            Health::Healthy => "stroke-success/55",
            Health::Degraded => "stroke-warning/80",
            Health::Down => "stroke-danger/85",
            Health::Offline | Health::Unknown => "stroke-text-faint",
        }
    }

    fn node(self) -> &'static str {
        match self {
            Health::Degraded => "fill-warning-weak stroke-warning",
            Health::Down => "fill-danger-weak stroke-danger",
            _ => "fill-surface stroke-border-strong dark:fill-surface-3",
        }
    }
}

#[cfg(test)]
mod tests;
