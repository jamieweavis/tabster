//! Renders a [`DisplayList`] to SVG: a development aid for eyeballing layout output without the
//! Swift shell. It paints exactly what the shell paints, with the same conventions.

use std::fmt::Write;

use super::{DisplayList, Ink, Primitive, TextAnchor};

fn colour(ink: Ink) -> &'static str {
    match ink {
        Ink::Primary => "#1d1d1f",
        Ink::Secondary => "#8a8a8e",
        Ink::Accent => "#d9480f",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// SVG document for a display list (white background, monospaced-digit font).
pub fn to_svg(list: &DisplayList) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="Menlo, SF Mono, monospace">"#,
        w = list.width,
        h = list.height
    );
    let _ = writeln!(out, r#"<rect width="100%" height="100%" fill="white"/>"#);
    for system in &list.systems {
        for p in &system.primitives {
            let _ = match p {
                Primitive::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    ink,
                } => writeln!(
                    out,
                    r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{}" stroke-width="{width}"/>"#,
                    colour(*ink)
                ),
                Primitive::Rect { rect, ink } => writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    colour(*ink)
                ),
                Primitive::Knockout { rect } => writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="white"/>"#,
                    rect.x, rect.y, rect.width, rect.height
                ),
                Primitive::Text {
                    x,
                    y,
                    text,
                    size,
                    bold,
                    italic,
                    anchor,
                    ink,
                } => {
                    let (a, baseline) = match anchor {
                        TextAnchor::Center => ("middle", "central"),
                        TextAnchor::Leading => ("start", "alphabetic"),
                    };
                    writeln!(
                        out,
                        r#"<text x="{x}" y="{y}" font-size="{size}" font-weight="{}" font-style="{}" text-anchor="{a}" dominant-baseline="{baseline}" fill="{}">{}</text>"#,
                        if *bold { "bold" } else { "normal" },
                        if *italic { "italic" } else { "normal" },
                        colour(*ink),
                        escape(text)
                    )
                }
                Primitive::Curve {
                    x1,
                    y1,
                    cx,
                    cy,
                    x2,
                    y2,
                    width,
                    ink,
                } => writeln!(
                    out,
                    r#"<path d="M{x1} {y1} Q{cx} {cy} {x2} {y2}" fill="none" stroke="{}" stroke-width="{width}"/>"#,
                    colour(*ink)
                ),
                Primitive::Dot { x, y, radius, ink } => writeln!(
                    out,
                    r#"<circle cx="{x}" cy="{y}" r="{radius}" fill="{}"/>"#,
                    colour(*ink)
                ),
            };
        }
    }
    out.push_str("</svg>\n");
    out
}
