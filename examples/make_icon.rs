//! Generates assets/icon.png — run: cargo run --release --example make_icon
//!
//! A glossy white beamed note floating over a dark vinyl record (fine
//! grooves + the classic diagonal sheen), on a macOS squircle tile with a
//! soft drop shadow. Monochrome. Rendered with 4×4 supersampling.

type Rgba = [f32; 4];

const S: f32 = 1024.0;

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

/// Signed distance to the macOS icon tile (824px, 185px corners).
fn tile_sd(x: f32, y: f32) -> f32 {
    let (half, r) = (412.0, 185.0);
    let dx = (x - 512.0).abs() - (half - r);
    let dy = (y - 512.0).abs() - (half - r);
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
}

/// Signed distance to a rotated ellipse (good near the edge).
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

/// The beamed double note.
fn note_sd(x: f32, y: f32) -> f32 {
    let rot = -0.42;
    let (rx, ry) = (84.0, 62.0);
    let (h1x, h1y, h2x, h2y) = (388.0, 694.0, 630.0, 638.0);
    let head1 = ellipse_sd(x, y, h1x, h1y, rx, ry, rot);
    let head2 = ellipse_sd(x, y, h2x, h2y, rx, ry, rot);
    let w = 32.0;
    // Stems rise from each head's right side.
    let (s1, s2) = (h1x + 50.0, h2x + 50.0);
    let stem1 = rect_sd(x, y, s1, 334.0, s1 + w, h1y - 20.0);
    let stem2 = rect_sd(x, y, s2, 278.0, s2 + w, h2y - 20.0);
    // Slanted beam joining the stem tops.
    let (bx0, by0, bx1, by1, thick) = (s1, 322.0, s2 + w, 266.0, 74.0);
    let top = by0 + (by1 - by0) * (x - bx0) / (bx1 - bx0);
    let slope = ((by1 - by0) / (bx1 - bx0)).atan().cos();
    let beam = ((top - y).max(y - (top + thick)) * slope).max(rect_sd(x, y, bx0, 0.0, bx1, S));
    head1.min(head2).min(stem1).min(stem2).min(beam)
}

fn over(dst: Rgba, src: Rgba) -> Rgba {
    let a = src[3] + dst[3] * (1.0 - src[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = (src[i] * src[3] + dst[i] * dst[3] * (1.0 - src[3])) / a;
    }
    out[3] = a;
    out
}

fn shade(x: f32, y: f32) -> Rgba {
    let mut px: Rgba = [0.0; 4];

    // Soft shadow under the tile.
    let sh = tile_sd(x, y - 14.0);
    let sh_a = 0.42 * (1.0 - smooth(-30.0, 34.0, sh));
    px = over(px, [0.0, 0.0, 0.0, sh_a]);

    let tsd = tile_sd(x, y);
    let cover = (0.5 - tsd).clamp(0.0, 1.0);
    if cover <= 0.0 {
        return px;
    }

    // Tile: deep graphite fading to black, lit from above.
    let t = (y - 100.0) / 824.0;
    let mut c = mix(hex(0x2b2b30), hex(0x050506), smooth(0.0, 1.0, t));
    let glow = ((x - 512.0).powi(2) + ((y - 120.0) * 1.3).powi(2)).sqrt();
    c = mix(c, hex(0x4a4a52), 0.45 * (1.0 - smooth(0.0, 560.0, glow)));

    // Vinyl record, centred a little low; the note floats over it.
    let (vx, vy) = (512.0, 560.0);
    let (dx, dy) = (x - vx, y - vy);
    let r = (dx * dx + dy * dy).sqrt();
    let disc_r = 352.0;
    if r < disc_r + 2.0 {
        let inside = (disc_r - r + 0.5).clamp(0.0, 1.0);
        let ang = dy.atan2(dx);
        // Grooves: fine rings with a slow variation so they don't moiré.
        let groove = 0.5 + 0.5 * (r * 0.9 + (r * 0.031).sin() * 4.0).sin();
        let groove_zone = smooth(120.0, 140.0, r) * (1.0 - smooth(disc_r - 26.0, disc_r - 14.0, r));
        let mut d = mix(hex(0x0c0c0e), hex(0x1a1a1e), groove * groove_zone);
        // The diagonal sheen vinyl gets under a light.
        let sheen = (2.0 * (ang + 0.75)).cos().max(0.0).powf(6.0);
        let sheen2 = (2.0 * (ang + 0.75) + 0.35).cos().max(0.0).powf(14.0);
        d = mix(d, hex(0x8c8c98), (0.42 * sheen + 0.25 * sheen2) * groove_zone * (0.55 + 0.45 * groove));
        // Label + spindle.
        let label = 1.0 - smooth(116.0, 119.0, r);
        d = mix(d, mix(hex(0x2e2e34), hex(0x1c1c20), (dy + 118.0) / 236.0), label);
        d = mix(d, hex(0x050506), 1.0 - smooth(9.0, 11.0, r));
        // Rim highlight.
        let rim = (1.0 - ((r - (disc_r - 1.5)).abs() / 1.5).min(1.0)) * (0.5 + 0.5 * (-dy / r.max(1.0)));
        d = mix(d, hex(0x8a8a94), 0.5 * rim);
        c = mix(c, d, inside);
    }

    // Inner top rim on the tile.
    if tsd > -3.0 && y < 512.0 {
        c = mix(c, hex(0xffffff), 0.10 * ((tsd + 3.0) / 3.0).clamp(0.0, 1.0));
    }

    // Note shadow: offset and blurred.
    let ns = note_sd(x - 6.0, y - 22.0);
    c = mix(c, hex(0x000000), 0.55 * (1.0 - smooth(-14.0, 30.0, ns)));

    // Note: glossy white with a cool grey toward the bottom.
    let nsd = note_sd(x, y);
    let ncover = (0.5 - nsd).clamp(0.0, 1.0);
    if ncover > 0.0 {
        let g = smooth(250.0, 760.0, y);
        let mut n = mix(hex(0xffffff), hex(0xd2d3da), g);
        // Soft inner edge so it reads as a solid, lit object.
        n = mix(n, hex(0x9a9ba4), 0.35 * (1.0 - smooth(-10.0, -1.0, nsd)) * smooth(400.0, 760.0, y));
        n = mix(n, hex(0xffffff), 0.6 * (1.0 - smooth(-6.0, -1.0, nsd)) * (1.0 - smooth(260.0, 520.0, y)));
        c = mix(c, n, ncover);
    }

    c[3] = cover;
    over(px, c)
}

fn main() {
    let n = 1024u32;
    let ss = 4;
    let mut img = image::RgbaImage::new(n, n);
    for py in 0..n {
        for px in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = px as f32 + (sx as f32 + 0.5) / ss as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / ss as f32;
                    let c = shade(x, y);
                    // Premultiply while averaging.
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let k = (ss * ss) as f32;
            let a = acc[3] / k;
            let rgb: [u8; 3] = std::array::from_fn(|i| if a > 0.0 { (acc[i] / k / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 });
            img.put_pixel(px, py, image::Rgba([rgb[0], rgb[1], rgb[2], (a * 255.0).round() as u8]));
        }
    }
    img.save("assets/icon.png").expect("save icon");
    println!("wrote assets/icon.png");
}
