//! Tab layout: turns one track of a [`Score`] into a [`DisplayList`] of positioned drawing
//! primitives that the Swift shell renders verbatim (no layout decisions in Swift).
//!
//! Coordinates are in points, origin top-left, y down. The layout is one continuous vertical
//! strip of systems (rows of bars) `options.width` wide; the shell scrolls it and only draws
//! systems that intersect the viewport.
//!
//! Frozen contract: the public types and `layout()` signature. Owner: Phase 1 agent D.

use crate::model::Score;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LayoutOptions {
    /// Index into `Score::tracks`.
    pub track: u32,
    /// Available width in points.
    pub width: f32,
    /// Zoom factor; 1.0 = default size (≈ 12pt fret numbers, 13pt string spacing).
    pub scale: f32,
    /// Draw rhythm stems/beams below the tab staff.
    pub show_rhythm: bool,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct DisplayList {
    pub width: f32,
    pub height: f32,
    /// Top to bottom.
    pub systems: Vec<System>,
}

/// One row of bars.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct System {
    /// Bounding box in list coordinates.
    pub frame: Rect,
    /// Everything drawn for this system, in list coordinates, in paint order.
    pub primitives: Vec<Primitive>,
    pub bars: Vec<BarBox>,
}

/// Hit-testing and cursor geometry for one master bar.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct BarBox {
    pub master_bar: u32,
    /// The bar's area on the tab staff, in list coordinates.
    pub frame: Rect,
    /// Beat onsets in the bar, ascending by tick.
    pub beats: Vec<BeatBox>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, uniffi::Record)]
pub struct BeatBox {
    pub tick_in_bar: u32,
    /// x of the beat's column centre, in list coordinates.
    pub x: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, uniffi::Record)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Enum)]
pub enum Ink {
    /// Primary foreground (staff lines, fret numbers).
    #[default]
    Primary,
    /// De-emphasised (bar numbers, ghost notes, technique labels).
    Secondary,
    /// Accent colour (section markers).
    Accent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TextAnchor {
    /// (x, y) is the centre of the text.
    Center,
    /// (x, y) is the left end of the baseline.
    Leading,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum Primitive {
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        ink: Ink,
    },
    /// Filled rectangle (beams, fret-number knockouts use `Knockout`).
    Rect { rect: Rect, ink: Ink },
    /// Background-coloured rectangle, drawn behind fret numbers so staff lines don't cross them.
    Knockout { rect: Rect },
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        bold: bool,
        italic: bool,
        anchor: TextAnchor,
        ink: Ink,
    },
    /// Quadratic curve (ties, slurs, hammer-on/pull-off arcs, bends).
    Curve {
        x1: f32,
        y1: f32,
        cx: f32,
        cy: f32,
        x2: f32,
        y2: f32,
        width: f32,
        ink: Ink,
    },
    /// Filled circle (rhythm dots, dot ornaments).
    Dot {
        x: f32,
        y: f32,
        radius: f32,
        ink: Ink,
    },
}

/// The playback cursor: a vertical bar spanning the tab staff of one system.
#[derive(Debug, Clone, Copy, Default, PartialEq, uniffi::Record)]
pub struct Cursor {
    pub master_bar: u32,
    /// Index into `DisplayList::systems`.
    pub system: u32,
    /// The cursor line's x, and the vertical extent of the staff it sits on.
    pub x: f32,
    pub y: f32,
    pub height: f32,
    /// The bar being played (for highlighting).
    pub bar_frame: Rect,
}

/// Lay out one track. An out-of-range track produces an empty list.
pub fn layout(score: &Score, options: &LayoutOptions) -> DisplayList {
    let _ = (score, options);
    todo!("Phase 1 agent D")
}

/// Cursor for a position `tick_in_bar` ticks into `master_bar`, whose actual length is
/// `bar_ticks`. Interpolates x linearly between beat onsets (and the bar's right edge after the
/// last beat), so the cursor glides smoothly. `None` if the bar isn't in the list.
/// Hot path (called every frame): no allocation, O(log n) lookup.
pub fn cursor(
    list: &DisplayList,
    master_bar: u32,
    tick_in_bar: u32,
    bar_ticks: u32,
) -> Option<Cursor> {
    let _ = (list, master_bar, tick_in_bar, bar_ticks);
    todo!("Phase 1 agent D")
}

/// The master bar and tick under a point in list coordinates, for click-to-seek. Snaps to the
/// nearest beat onset in the bar under the point. `None` if no bar is there.
pub fn hit_test(list: &DisplayList, x: f32, y: f32) -> Option<(u32, u32)> {
    let _ = (list, x, y);
    todo!("Phase 1 agent D")
}
