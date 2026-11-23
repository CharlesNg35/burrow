//! Windowing for long scrolled content: draw only what is in and near the view.
//! `range` is the pure math (which items, how much space above and below), `track` follows a
//! scrolling element, and `list` is the keyed list component built on both.

mod list;
mod range;
mod track;

pub use list::VirtualList;
pub use range::{Band, Snap, Span, Viewport, offset, span};
pub use track::{Scroller, Tracked, track};
