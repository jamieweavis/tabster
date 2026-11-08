//! Render one track of a tab file to SVG: `cargo run --example render_svg -- <file> <track> <out.svg> [width]`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path, track, out, rest @ ..] = args.as_slice() else {
        eprintln!("usage: render_svg <file> <track> <out.svg> [width]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("read file");
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());
    let score = tabster_core::formats::parse(&bytes, ext).expect("parse");
    let options = tabster_core::layout::LayoutOptions {
        track: track.parse().expect("track index"),
        width: rest.first().map_or(1000.0, |w| w.parse().expect("width")),
        scale: 1.0,
        show_rhythm: true,
    };
    let list = tabster_core::layout::layout(&score, &options);
    std::fs::write(out, tabster_core::layout::svg::to_svg(&list)).expect("write svg");
    eprintln!("{} systems, {:.0}pt tall", list.systems.len(), list.height);
}
