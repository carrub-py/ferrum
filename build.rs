use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));

    // Generate crisp 32x32 minimalist obsidian-and-silver Ferrum Shield icon
    let (rgba, bgra_bottom_up) = render_ferrum_icon_32();
    let rgba_path = out_dir.join("ferrum_icon_32.rgba");
    fs::write(&rgba_path, &rgba).expect("Failed to write RGBA icon");

    let ico_bytes = build_ico_32(&bgra_bottom_up);
    let ico_path = out_dir.join("ferrum.ico");
    fs::write(&ico_path, &ico_bytes).expect("Failed to write ICO icon");

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().unwrap());
        res.set("ProductName", "Ferrum Browser");
        res.set(
            "FileDescription",
            "Ferrum Browser — Navegador web minimalista y seguro en Rust",
        );
        res.set("LegalCopyright", "Ferrum Browser");
        let _ = res.compile();
    }

    println!("cargo:rerun-if-changed=build.rs");
}

fn render_ferrum_icon_32() -> (Vec<u8>, Vec<u8>) {
    const W: usize = 32;
    const H: usize = 32;
    let mut rgba = vec![0u8; W * H * 4];

    for y in 0..H {
        for x in 0..W {
            let fx = (x as f32 + 0.5) / W as f32 * 2.0 - 1.0; // -1..1
            let fy = (y as f32 + 0.5) / H as f32 * 2.0 - 1.0;

            // Squircle SDF (rounded Apple app-icon silhouette)
            let ax = fx.abs();
            let ay = fy.abs();
            let r = 0.38_f32;
            let bx = (ax - (0.92 - r)).max(0.0);
            let by = (ay - (0.92 - r)).max(0.0);
            let dist = (bx * bx + by * by).sqrt() - r;

            let idx = (y * W + x) * 4;
            if dist > 0.04 {
                continue;
            }

            let alpha = ((0.04 - dist) / 0.08).clamp(0.0, 1.0);

            // Base obsidian vertical gradient (#16161c -> #050507)
            let t = (fy + 1.0) * 0.5;
            let mut cr = 24.0 * (1.0 - t) + 6.0 * t;
            let mut cg = 24.0 * (1.0 - t) + 6.0 * t;
            let mut cb = 30.0 * (1.0 - t) + 9.0 * t;

            // Subtle hairline specular rim
            if dist > -0.07 {
                let rim = if fy < 0.0 { 48.0 } else { 22.0 };
                cr += rim;
                cg += rim;
                cb += rim + 4.0;
            }

            // Minimalist silver shield emblem in the center
            let sx = fx.abs();
            let sy = fy;
            let in_shield = if (-0.54..=0.58).contains(&sy) {
                let top_curve = -0.54 + sx * 0.22;
                let max_w = if sy < 0.05 {
                    0.46
                } else {
                    let k = ((sy - 0.05) / 0.53).clamp(0.0, 1.0);
                    0.46 * (1.0 - k * k)
                };
                sy >= top_curve && sx <= max_w
            } else {
                false
            };

            let in_inner = if (-0.40..=0.42).contains(&sy) {
                let top_curve = -0.40 + sx * 0.20;
                let max_w = if sy < 0.04 {
                    0.32
                } else {
                    let k = ((sy - 0.04) / 0.38).clamp(0.0, 1.0);
                    0.32 * (1.0 - k * k)
                };
                sy >= top_curve && sx <= max_w
            } else {
                false
            };

            if in_shield && !in_inner {
                let lum = 242.0 * (1.0 - t * 0.28);
                cr = lum;
                cg = lum;
                cb = (lum + 6.0).min(255.0);
            } else if in_inner && fx < 0.0 {
                // Subtle glass bevel inside left half of shield
                cr = (cr + 16.0).min(255.0);
                cg = (cg + 16.0).min(255.0);
                cb = (cb + 22.0).min(255.0);
            }

            rgba[idx] = cr.clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = cg.clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = cb.clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = (alpha * 255.0) as u8;
        }
    }

    // Bottom-up BGRA for Windows ICO DIB
    let mut bgra = Vec::with_capacity(W * H * 4);
    for y in (0..H).rev() {
        for x in 0..W {
            let idx = (y * W + x) * 4;
            bgra.push(rgba[idx + 2]);
            bgra.push(rgba[idx + 1]);
            bgra.push(rgba[idx]);
            bgra.push(rgba[idx + 3]);
        }
    }

    (rgba, bgra)
}

fn build_ico_32(bgra_bottom_up: &[u8]) -> Vec<u8> {
    const W: u32 = 32;
    const H: u32 = 32;
    let mask_size = (W / 8 * H) as usize; // 128 bytes AND mask
    let dib_size = 40 + bgra_bottom_up.len() + mask_size;

    let mut out = Vec::with_capacity(22 + dib_size);
    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = ICO
    out.extend_from_slice(&1u16.to_le_bytes()); // count = 1

    // ICONDIRENTRY
    out.push(W as u8);
    out.push(H as u8);
    out.push(0); // color palette
    out.push(0); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // color planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&(dib_size as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes()); // offset

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(W as i32).to_le_bytes());
    out.extend_from_slice(&((H * 2) as i32).to_le_bytes()); // height * 2 (XOR + AND)
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(bgra_bottom_up.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // ppm + colors

    out.extend_from_slice(bgra_bottom_up);
    out.extend(std::iter::repeat_n(0u8, mask_size));
    out
}
