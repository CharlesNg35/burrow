//! Generic Leptos components for the admin UI: inputs, tables, charts, dialogs and path diagrams.
//! It knows nothing about burrow and must not depend on burrow-admin-proto, burrow-syntax or burrow-admin-logic.

#![forbid(unsafe_code)]

mod bar;
mod basic;
mod code;
mod combo;
mod disclosure;
mod feedback;
mod field;
mod icon;
mod input;
mod overlay;
mod path;
mod radio;
mod scroll;
mod spark;
mod stepper;
mod toast;

pub use bar::{StickyBar, bar_status};
pub use basic::{
    Button, CARD_VIEW, Card, Chip, Fact, Facts, IconButton, Kbd, Kpi, LABEL, LINK, LockLine,
    MENU_ITEM, MONO, Mono, Muted, PageHeader, PillTabs, SectionHead, SpinDot, Stat, Stats,
    StatusDot, TABLE_VIEW, TabMark, Table, Tabs, Tag, Tone, WIDE_ONLY, button_class, card_head,
    chip_class, menu_class,
};
pub use code::{CodeBlock, DiffLine, DiffView, copy_text, download, download_text};
pub use combo::{ComboOption, Combobox, filter_options};
pub use disclosure::Disclosure;
pub use feedback::{Banner, Callout, DANGER_CARD, EmptyState, Skeleton, Spinner};
pub use field::{FieldRow, FieldState, field_message, tag_class};
pub use icon::{Icon, path as icon_path};
pub use input::{
    ChipInput, ConfirmInput, INPUT_H, Segmented, Select, TextArea, TextInput, Toggle, field_class,
    split_entries,
};
pub use overlay::{
    Dialog, Drawer, Tooltip, active as active_element, focus_first, trap as trap_focus,
};
pub use path::{Edge, EdgeState, Hop, HopState, PathData, PathDiagram};
pub use radio::{RadioList, RadioOption};
pub use scroll::{ScrollRow, active_item};
pub use spark::{ChartData, LineChart, Sparkline, segments, spark_max, spark_path};
pub use stepper::{Stepper, stepped};
pub use toast::{Toast, ToastHost};
