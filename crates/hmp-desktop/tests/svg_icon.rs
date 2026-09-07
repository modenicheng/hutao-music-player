//! SVG 图标栅格化形状抽查：把 24×24 路径渲染成 alpha 网格打印，
//! 供人工核对路径是否被正确解析（排查 ✕ 渲染成乱形）。
fn raster(path: &str) {
    let data = std::fs::read(path).unwrap();
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).unwrap();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(24, 24).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    println!("== {path}");
    for y in 0..24 {
        let mut line = String::new();
        for x in 0..24 {
            let a = pixmap.data()[(y * 24 + x) * 4 + 3];
            line.push(if a > 128 {
                '#'
            } else if a > 40 {
                '.'
            } else {
                ' '
            });
        }
        println!("{line}");
    }
}

#[test]
fn dump_icons() {
    let base = format!("{}/ui/assets/icons/", env!("CARGO_MANIFEST_DIR"));
    for name in [
        "close-rounded.svg",
        "home-rounded.svg",
        "expand-more-rounded.svg",
    ] {
        raster(&format!("{base}{name}"));
    }
}
