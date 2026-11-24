use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));

    // High-DPI 64x64 RGBA icon for taskbar, Alt+Tab, and window titlebar
    let rgba_64 = render_ferrum_icon_rgba(64);
    fs::write(out_dir.join("ferrum_icon_64.rgba"), &rgba_64)
        .expect("Failed to write 64x64 RGBA icon");

    // Multi-resolution HD Windows ICO (16, 24, 32, 48, 64, 128 DIB + 256 PNG)
    let ico_bytes = build_multi_res_ico(&[16, 24, 32, 48, 64, 128, 256]);
    let ico_path = out_dir.join("ferrum.ico");
    fs::write(&ico_path, &ico_bytes).expect("Failed to write multi-res ICO icon");

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().unwrap());
        res.set("ProductName", "Ferrum Browser");
        res.set(
            "FileDescription",
            "Ferrum Browser — Navegador web minimalista y seguro en Rust",
        );
        res.set(
            "LegalCopyright",
            "Copyright (c) 2026 carrub-py (MIT License)",
        );
        let _ = res.compile();
    }

    println!("cargo:rerun-if-changed=build.rs");
}

/// Renders an anti-aliased (4x4 supersampled) Apple Obsidian + Silver Shield icon at `size x size`.
fn render_ferrum_icon_rgba(size: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; size * size * 4];
    let sub: usize = if size <= 64 { 4 } else { 2 };
    let inv_samples = 1.0 / ((sub * sub) as f32);

    for y in 0..size {
        for x in 0..size {
            let mut acc_r = 0.0_f32;
            let mut acc_g = 0.0_f32;
            let mut acc_b = 0.0_f32;
            let mut acc_a = 0.0_f32;

            for sy in 0..sub {
                for sx in 0..sub {
                    let fx = (x as f32 + (sx as f32 + 0.5) / sub as f32) / size as f32 * 2.0 - 1.0;
                    let fy = (y as f32 + (sy as f32 + 0.5) / sub as f32) / size as f32 * 2.0 - 1.0;
                    let (r, g, b, a) = sample_ferrum_pixel(fx, fy);
                    acc_r += r * a;
                    acc_g += g * a;
                    acc_b += b * a;
                    acc_a += a;
                }
            }

            let a = (acc_a * inv_samples).clamp(0.0, 1.0);
            let idx = (y * size + x) * 4;
            if a > 1e-4 {
                let norm = inv_samples / a;
                rgba[idx] = (acc_r * norm).clamp(0.0, 255.0) as u8;
                rgba[idx + 1] = (acc_g * norm).clamp(0.0, 255.0) as u8;
                rgba[idx + 2] = (acc_b * norm).clamp(0.0, 255.0) as u8;
                rgba[idx + 3] = (a * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }

    rgba
}

fn sample_ferrum_pixel(fx: f32, fy: f32) -> (f32, f32, f32, f32) {
    // Squircle SDF (rounded Apple app-icon silhouette)
    let ax = fx.abs();
    let ay = fy.abs();
    let r = 0.38_f32;
    let bx = (ax - (0.93 - r)).max(0.0);
    let by = (ay - (0.93 - r)).max(0.0);
    let dist = (bx * bx + by * by).sqrt() - r;

    if dist > 0.03 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let alpha = ((0.03 - dist) / 0.06).clamp(0.0, 1.0);

    // Deep Apple Obsidian vertical sheen (#1a1a22 -> #050508)
    let t = (fy + 1.0) * 0.5;
    let radial = (fx * fx + (fy + 0.45) * (fy + 0.45)).sqrt();
    let top_glow = (1.0 - radial * 0.72).clamp(0.0, 1.0) * 14.0;
    let mut cr = 24.0 * (1.0 - t) + 6.0 * t + top_glow;
    let mut cg = 24.0 * (1.0 - t) + 6.0 * t + top_glow;
    let mut cb = 31.0 * (1.0 - t) + 9.0 * t + top_glow * 1.15;

    // Hairline specular border
    if dist > -0.055 {
        let rim = if fy < 0.0 { 54.0 } else { 22.0 };
        cr += rim;
        cg += rim;
        cb += rim + 5.0;
    }

    // Minimalist forged-silver shield emblem
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
        let lum = 246.0 * (1.0 - t * 0.26);
        cr = lum;
        cg = lum;
        cb = (lum + 7.0).min(255.0);
    } else if in_inner {
        if fx < 0.0 {
            cr = (cr + 18.0).min(255.0);
            cg = (cg + 18.0).min(255.0);
            cb = (cb + 24.0).min(255.0);
        } else {
            cr = (cr + 6.0).min(255.0);
            cg = (cg + 6.0).min(255.0);
            cb = (cb + 10.0).min(255.0);
        }
    }

    (cr, cg, cb, alpha)
}

fn build_multi_res_ico(sizes: &[usize]) -> Vec<u8> {
    let mut payloads: Vec<(usize, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &sz in sizes {
        let rgba = render_ferrum_icon_rgba(sz);
        let data = if sz >= 256 {
            encode_rgba_png(&rgba, sz as u32, sz as u32)
        } else {
            encode_ico_dib(&rgba, sz)
        };
        payloads.push((sz, data));
    }

    let count = payloads.len();
    let header_size = 6 + count * 16;
    let total_payload: usize = payloads.iter().map(|(_, d)| d.len()).sum();
    let mut out = Vec::with_capacity(header_size + total_payload);

    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = ICO
    out.extend_from_slice(&(count as u16).to_le_bytes());

    // ICONDIRENTRY table
    let mut offset = header_size as u32;
    for (sz, data) in &payloads {
        let dim_byte = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim_byte);
        out.push(dim_byte);
        out.push(0); // palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }

    for (_, data) in payloads {
        out.extend_from_slice(&data);
    }
    out
}

fn encode_ico_dib(rgba: &[u8], size: usize) -> Vec<u8> {
    let w = size as u32;
    let h = size as u32;
    let mut bgra = Vec::with_capacity(size * size * 4);
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            bgra.push(rgba[idx + 2]);
            bgra.push(rgba[idx + 1]);
            bgra.push(rgba[idx]);
            bgra.push(rgba[idx + 3]);
        }
    }
    // AND mask rows must be padded to 4-byte boundaries
    let mask_row_bytes = size.div_ceil(32) * 4;
    let mask_size = mask_row_bytes * size;

    let mut dib = Vec::with_capacity(40 + bgra.len() + mask_size);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(w as i32).to_le_bytes());
    dib.extend_from_slice(&((h * 2) as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib.extend_from_slice(&(bgra.len() as u32).to_le_bytes());
    dib.extend_from_slice(&[0u8; 16]);
    dib.extend_from_slice(&bgra);
    dib.extend(std::iter::repeat_n(0u8, mask_size));
    dib
}

/// Minimal zero-dependency valid RGBA PNG encoder (stored zlib blocks) for 256x256 ICO embedding.
fn encode_rgba_png(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row_len = (w as usize) * 4;
    let mut raw = Vec::with_capacity((row_len + 1) * (h as usize));
    for y in 0..(h as usize) {
        raw.push(0u8); // filter type 0 (None)
        raw.extend_from_slice(&rgba[y * row_len..(y + 1) * row_len]);
    }

    // Wrap raw scanlines in valid uncompressed zlib stream (RFC 1950 + stored DEFLATE blocks)
    let mut zlib = Vec::with_capacity(raw.len() + 64);
    zlib.push(0x78); // CMF: deflate, 32K window
    zlib.push(0x01); // FLG: no dict, check bits valid (0x7801 % 31 == 0)

    let mut pos = 0usize;
    while pos < raw.len() {
        let chunk_len = (raw.len() - pos).min(65535);
        let is_final = pos + chunk_len == raw.len();
        zlib.push(if is_final { 0x01 } else { 0x00 });
        let len16 = chunk_len as u16;
        zlib.extend_from_slice(&len16.to_le_bytes());
        zlib.extend_from_slice(&(!len16).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + chunk_len]);
        pos += chunk_len;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(zlib.len() + 128);
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    push_png_chunk(&mut png, b"IHDR", &ihdr);
    push_png_chunk(&mut png, b"IDAT", &zlib);
    push_png_chunk(&mut png, b"IEND", &[]);
    png
}

fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32_update(0xFFFF_FFFF, kind);
    crc = crc32_update(crc, data);
    out.extend_from_slice(&(!crc).to_be_bytes());
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in bytes {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}
