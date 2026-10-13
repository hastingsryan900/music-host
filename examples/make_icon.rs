//! Generates assets/icon.png — run: cargo run --release --example make_icon
//!
//! A white beamed double note (Apple Music–style) on a black tile with a soft
//! graphite gradient. Rendered with 4×4 supersampling.

type Rgba = [f32; 4];

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn hex(h: u32) -> Rgba {
    [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0, 1.0]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance to the macOS-style rounded tile.
fn tile_sd(x: f32, y: f32) -> f32 {
    let (inset, r) = (100.0, 186.0);
    let half = 512.0 - inset;
    let dx = (x - 512.0).abs() - (half - r);
    let dy = (y - 512.0).abs() - (half - r);
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
}

/// Signed distance to a rotated ellipse (approximate, good near the edge).
fn ellipse_sd(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32, rot: f32) -> f32 {
    let (s, c) = rot.sin_cos();
    let (dx, dy) = (x - cx, y - cy);
    let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
    let k = ((u / rx).powi(2) + (v / ry).powi(2)).sqrt();
    (k - 1.0) * rx.min(ry)
}

fn rect_sd(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let dx = (x0 - x).max(x - x1);
    let dy = (y0 - y).max(y - y1);
    dx.max(dy).min(0.0) + (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()
}

/// The double note: two tilted heads, two stems, one slanted beam.
fn note_sd(x: f32, y: f32) -> f32 {
    let rot = -0.38;
    let head1 = ellipse_sd(x, y, 398.0, 712.0, 96.0, 70.0, rot);
    let head2 = ellipse_sd(x, y, 648.0, 652.0, 96.0, 70.0, rot);
    let stem_w = 36.0;
    // Stems sit on the heads' right edges and start under the beam.
    let stem1 = rect_sd(x, y, 452.0, 330.0, 452.0 + stem_w, 684.0);
    let stem2 = rect_sd(x, y, 702.0, 272.0, 702.0 + stem_w, 622.0);
    // Beam: parallelogram from the left stem's top to the right stem's top.
    let (bx0, by0, bx1, by1, thick) = (452.0, 318.0, 738.0, 258.0, 84.0);
    let beam = if x < bx0 - 2.0 || x > bx1 + 2.0 {
        rect_sd(x, y, bx0, by1, bx1, by0 + thick)
    } else {
        let top = by0 + (by1 - by0) * (x - bx0) / (bx1 - bx0);
        (top - y).max(y - (top + thick)) * 0.97
    };
    let beam = beam.max(rect_sd(x, y, bx0, 0.0, bx1, 1024.0));
    head1.min(head2).min(stem1).min(stem2).min(beam)
}

fn shade(x: f32, y: f32) -> Rgba {
    let tsd = tile_sd(x, y);
    if tsd > 1.0 {
        return [0.0; 4];
    }
    // Graphite → black, with a soft light from the top.
    let t = (y - 100.0) / 824.0;
    let mut c = mix(hex(0x3a3a3e), hex(0x070708), smooth(0.0, 1.0, t));
    let glow = ((x - 512.0).powi(2) + (y - 160.0).powi(2)).sqrt();
    c = mix(c, hex(0x56565c), 0.35 * (1.0 - smooth(0.0, 520.0, glow)));
    // Thin bright rim at the top edge.
    if -tsd < 3.0 && y < 512.0 {
        c = mix(c, hex(0xffffff), 0.10 * (1.0 + tsd / 3.0).clamp(0.0, 1.0));
    }
    // Soft shadow under the note, then the note itself.
    let sh = note_sd(x - 6.0, y - 14.0);
    c = mix(c, hex(0x000000), 0.45 * (1.0 - smooth(-10.0, 40.0, sh)));
    let n = note_sd(x, y);
    let cover = 1.0 - smooth(-0.75, 0.75, n);
    let white = mix(hex(0xffffff), hex(0xdedee2), smooth(240.0, 790.0, y));
    c = mix(c, white, cover);
    let a = 1.0 - smooth(-0.75, 0.75, tsd);
    [c[0], c[1], c[2], a]
}

fn main() {
    let n = 1024u32;
    let ss = 4;
    let mut img = image::RgbaImage::new(n, n);
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = x as f32 + (sx as f32 + 0.5) / ss as f32;
                    let fy = y as f32 + (sy as f32 + 0.5) / ss as f32;
                    let c = shade(fx, fy);
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let a = acc[3] / (ss * ss) as f32;
            let px = if acc[3] > 0.0 {
                [(acc[0] / acc[3] * 255.0) as u8, (acc[1] / acc[3] * 255.0) as u8, (acc[2] / acc[3] * 255.0) as u8, (a * 255.0) as u8]
            } else {
                [0, 0, 0, 0]
            };
            img.put_pixel(x, y, image::Rgba(px));
        }
    }
    img.save("assets/icon.png").unwrap();
}
