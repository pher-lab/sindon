//! image_demo — exercise the Phase 36 B-3 Image widget.
//!
//! Builds three test images at startup (no on-disk assets), then shows
//! each one under all four `ImageFit` modes plus a tinted variant. The
//! window deliberately gives every cell a tall, narrow aspect so the
//! difference between Contain / Cover / Fill / None is visually obvious
//! when the image is wider than its box.
//!
//! Visual check:
//! - **Contain**: image fits, letterbox stripes top/bottom (or sides).
//! - **Cover**: image fills the cell; edges crop.
//! - **Fill**: image stretches, ignoring its aspect ratio.
//! - **None**: image at intrinsic size, centered (clipped if too big).
//! - **Tinted**: 50% alpha tint fades the image.

use shroud::app::App;
use shroud::core::Color;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, Image, ImageFit, TextWidget};

fn make_gradient_png(width: u32, height: u32) -> Vec<u8> {
    // Cyan → magenta linear gradient with a diagonal red bar to make
    // crops/stretches conspicuous.
    let mut img = image::RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let t = x as f32 / (width as f32 - 1.0).max(1.0);
        let r = (40.0 + t * 200.0) as u8;
        let g = (160.0 - t * 100.0) as u8;
        let b = (220.0 - t * 80.0) as u8;
        let near_diag = (x as i32 - y as i32).abs() < 4;
        *px = if near_diag {
            image::Rgba([230, 70, 90, 255])
        } else {
            image::Rgba([r, g, b, 255])
        };
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn make_checker_jpeg(width: u32, height: u32) -> Vec<u8> {
    // JPEG path, distinct from the PNG decoder branch.
    let mut img = image::RgbImage::new(width, height);
    let cell = 16;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let on = ((x / cell) + (y / cell)) % 2 == 0;
        *px = if on {
            image::Rgb([230, 215, 180])
        } else {
            image::Rgb([60, 80, 120])
        };
    }
    let mut out = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut out),
        image::ImageFormat::Jpeg,
    )
    .unwrap();
    out
}

fn label(text: &str) -> TextWidget {
    TextWidget::new(text)
        .font_size(12.0)
        .color(Color::rgb(0.7, 0.75, 0.85))
}

fn main() {
    App::new()
        .title("shroud — image rendering demo")
        .size(900, 640)
        .run(|_scope| {
            // 64 wide x 96 tall PNG, JPEG checker 64x64.
            let png_bytes = make_gradient_png(64, 96);
            let jpg_bytes = make_checker_jpeg(64, 64);

            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(20.0)
                    .gap(14.0)
                    .background(Color::rgb(0.08, 0.08, 0.12)),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 36 \u{2014} Image rendering")
                    .font_size(22.0)
                    .color(Color::rgb(0.92, 0.94, 1.0)),
            );
            tree.add_child(
                root,
                TextWidget::new(
                    "Each cell is 140\u{00d7}90 px. The PNG inside is 64\u{00d7}96 (taller than \
                     wide); JPEG is 64\u{00d7}64 (square).",
                )
                .font_size(13.0)
                .color(Color::rgb(0.65, 0.7, 0.78)),
            );

            // ── Row of fit modes (PNG) ──────────────────────────────
            let header_png = tree.add_child(root, Container::row().gap(8.0).align_center());
            tree.add_child(
                header_png,
                TextWidget::new("PNG \u{2014} ImageFit modes:")
                    .font_size(14.0)
                    .color(Color::rgb(0.9, 0.92, 0.96)),
            );

            let png_row = tree.add_child(root, Container::row().gap(14.0).align_center());
            for (name, fit) in [
                ("Contain", ImageFit::Contain),
                ("Cover", ImageFit::Cover),
                ("Fill", ImageFit::Fill),
                ("None", ImageFit::None),
            ] {
                let cell = tree.add_child(
                    png_row,
                    Container::column()
                        .gap(4.0)
                        .padding(6.0)
                        .background(Color::rgb(0.14, 0.14, 0.20))
                        .radius(6.0)
                        .align_center(),
                );
                tree.add_child(cell, label(name));
                tree.add_child(
                    cell,
                    Image::from_bytes(&png_bytes)
                        .expect("png decodes")
                        .width(140.0)
                        .height(90.0)
                        .fit(fit),
                );
            }

            // ── Row of fit modes (JPEG) ─────────────────────────────
            let header_jpg = tree.add_child(root, Container::row().gap(8.0).align_center());
            tree.add_child(
                header_jpg,
                TextWidget::new("JPEG \u{2014} same modes (square source):")
                    .font_size(14.0)
                    .color(Color::rgb(0.9, 0.92, 0.96)),
            );

            let jpg_row = tree.add_child(root, Container::row().gap(14.0).align_center());
            for (name, fit) in [
                ("Contain", ImageFit::Contain),
                ("Cover", ImageFit::Cover),
                ("Fill", ImageFit::Fill),
                ("None", ImageFit::None),
            ] {
                let cell = tree.add_child(
                    jpg_row,
                    Container::column()
                        .gap(4.0)
                        .padding(6.0)
                        .background(Color::rgb(0.14, 0.14, 0.20))
                        .radius(6.0)
                        .align_center(),
                );
                tree.add_child(cell, label(name));
                tree.add_child(
                    cell,
                    Image::from_bytes(&jpg_bytes)
                        .expect("jpeg decodes")
                        .width(140.0)
                        .height(90.0)
                        .fit(fit),
                );
            }

            // ── Tint row ────────────────────────────────────────────
            let header_tint = tree.add_child(root, Container::row().gap(8.0).align_center());
            tree.add_child(
                header_tint,
                TextWidget::new("Tint multiplies sampled RGBA (default = WHITE):")
                    .font_size(14.0)
                    .color(Color::rgb(0.9, 0.92, 0.96)),
            );

            let tint_row = tree.add_child(root, Container::row().gap(14.0).align_center());
            for (name, tint) in [
                ("White (no-op)", Color::WHITE),
                ("Alpha 0.5", Color::rgba(1.0, 1.0, 1.0, 0.5)),
                ("Red tint", Color::rgba(1.0, 0.4, 0.4, 1.0)),
                ("Blue tint", Color::rgba(0.4, 0.6, 1.0, 1.0)),
            ] {
                let cell = tree.add_child(
                    tint_row,
                    Container::column()
                        .gap(4.0)
                        .padding(6.0)
                        .background(Color::rgb(0.14, 0.14, 0.20))
                        .radius(6.0)
                        .align_center(),
                );
                tree.add_child(cell, label(name));
                tree.add_child(
                    cell,
                    Image::from_bytes(&png_bytes)
                        .expect("png decodes")
                        .width(140.0)
                        .height(90.0)
                        .fit(ImageFit::Cover)
                        .tint(tint),
                );
            }

            tree
        });
}
