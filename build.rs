use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_root = PathBuf::from(&manifest_dir);
    let manifest = manifest_root.join("assets").join("app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    let rel_manifest = Path::new("assets").join("app.manifest");
    if rel_manifest.is_file() {
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:assets\\app.manifest");
    } else {
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }

    let sizes = [16u32, 32u32, 48u32];
    let dibs: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&sz| (sz, build_icon_dib(sz))).collect();

    let ico_path = manifest_root.join("assets").join("bbstats.ico");
    let ico_bytes = build_ico_file(&dibs);
    let needs_ico_write = fs::read(&ico_path)
        .map(|existing| existing != ico_bytes)
        .unwrap_or(true);
    if needs_ico_write {
        if let Some(parent) = ico_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&ico_path, &ico_bytes).expect("failed to write assets/bbstats.ico");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res_path = out_dir.join("bbstats.res");
    let res_bytes = build_win32_res(&dibs);
    fs::write(&res_path, &res_bytes).expect("failed to write bbstats.res");
    println!("cargo:rustc-link-arg={}", res_path.display());
}

#[allow(clippy::too_many_arguments)]
fn sdf_letter_b(
    fx: f32,
    fy: f32,
    stem_x: f32,
    top_y: f32,
    bot_y: f32,
    bowl_cx: f32,
    bowl_cy: f32,
    bowl_r: f32,
    stroke: f32,
) -> f32 {
    let half = stroke * 0.5;
    let y0 = top_y + half;
    let y1 = (bot_y - half).max(y0);
    let dy_stem = fy - fy.clamp(y0, y1);
    let d_stem = (fx - stem_x).hypot(dy_stem) - half;
    let d_bowl = ((fx - bowl_cx).hypot(fy - bowl_cy) - bowl_r).abs() - half;
    d_stem.min(d_bowl)
}

fn render_icon_bgra_top_down(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let margin = (s * 0.06).max(1.0);
    let half = s * 0.5 - margin;
    let radius = (s * 0.24).max(2.0);
    let cx = s * 0.5;
    let cy = s * 0.5;

    let stroke = (s * 0.088).max(1.4);
    let top_y = s * 0.22;
    let bot_y = s * 0.74;
    let bowl_cy = s * 0.58;
    let bowl_r = s * 0.115;
    let dot_cx = s * 0.80;
    let dot_cy = s * 0.26;
    let dot_r = (s * 0.055).max(1.1);

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;

            let qx = (fx - cx).abs() - (half - radius);
            let qy = (fy - cy).abs() - (half - radius);
            let sdf = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;

            let cov = (0.7 - sdf).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }

            let mut r = 16.0_f32;
            let mut g = 19.0_f32;
            let mut b = 26.0_f32;
            let alpha = (cov * 255.0).round().clamp(0.0, 255.0) as u8;

            // Metallic blue-teal top-rim bevel
            if sdf > -1.45 {
                let border_t = ((sdf + 1.45) / 1.45).clamp(0.0, 1.0);
                let top_mix = ((1.0 - fy / s) * 0.8 + 0.2).clamp(0.2, 1.0);
                r += (82.0 - r) * border_t * top_mix;
                g += (142.0 - g) * border_t * top_mix;
                b += (210.0 - b) * border_t * top_mix;
            }

            // Left 'b' in crisp ice white
            let b1_sdf = sdf_letter_b(
                fx,
                fy,
                s * 0.205,
                top_y,
                bot_y,
                s * 0.32,
                bowl_cy,
                bowl_r,
                stroke,
            );
            let b1_cov = (0.6 - b1_sdf).clamp(0.0, 1.0);
            if b1_cov > 0.0 {
                r += (242.0 - r) * b1_cov;
                g += (246.0 - g) * b1_cov;
                b += (252.0 - b) * b1_cov;
            }

            // Right 'b' in vertical emerald -> cyan-blue gradient
            let b2_sdf = sdf_letter_b(
                fx,
                fy,
                s * 0.565,
                top_y,
                bot_y,
                s * 0.68,
                bowl_cy,
                bowl_r,
                stroke,
            );
            let b2_cov = (0.6 - b2_sdf).clamp(0.0, 1.0);
            if b2_cov > 0.0 {
                let gt = ((fy - top_y) / (bot_y - top_y).max(1.0)).clamp(0.0, 1.0);
                let tr = 52.0 + (10.0 - 52.0) * gt;
                let tg = 211.0 + (132.0 - 211.0) * gt;
                let tb = 153.0 + (255.0 - 153.0) * gt;
                r += (tr - r) * b2_cov;
                g += (tg - g) * b2_cov;
                b += (tb - b) * b2_cov;
            }

            // Subtle emerald glow + live status dot in upper-right quadrant
            let d_dot = (fx - dot_cx).hypot(fy - dot_cy) - dot_r;
            let glow = (1.0 - (d_dot / (dot_r * 2.2))).clamp(0.0, 1.0) * 0.30;
            if glow > 0.0 {
                r = r * (1.0 - glow) + 52.0 * glow;
                g = g * (1.0 - glow) + 211.0 * glow;
                b = b * (1.0 - glow) + 153.0 * glow;
            }
            let dot_cov = (0.6 - d_dot).clamp(0.0, 1.0);
            if dot_cov > 0.0 {
                r += (52.0 - r) * dot_cov;
                g += (211.0 - g) * dot_cov;
                b += (153.0 - b) * dot_cov;
            }

            let idx = ((y * size + x) * 4) as usize;
            px[idx] = b.round().clamp(0.0, 255.0) as u8;
            px[idx + 1] = g.round().clamp(0.0, 255.0) as u8;
            px[idx + 2] = r.round().clamp(0.0, 255.0) as u8;
            px[idx + 3] = alpha;
        }
    }
    px
}

fn build_icon_dib(size: u32) -> Vec<u8> {
    let top_down = render_icon_bgra_top_down(size);
    let xor_size = (size * size * 4) as usize;
    let and_row = (size.div_ceil(32) * 4) as usize;
    let and_size = and_row * size as usize;

    let mut out = Vec::with_capacity(40 + xor_size + and_size);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (XOR + AND)
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&(xor_size as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    let stride = (size * 4) as usize;
    for y in (0..size as usize).rev() {
        let row = &top_down[y * stride..(y + 1) * stride];
        out.extend_from_slice(row);
    }
    out.resize(out.len() + and_size, 0);
    out
}

fn build_ico_file(dibs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(dibs.len() as u16).to_le_bytes());

    let mut offset = (6 + dibs.len() * 16) as u32;
    for (sz, dib) in dibs {
        let dim = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += dib.len() as u32;
    }
    for (_, dib) in dibs {
        out.extend_from_slice(dib);
    }
    out
}

fn push_res_entry(out: &mut Vec<u8>, type_id: u16, name_id: u16, mem_flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&type_id.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&name_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&mem_flags.to_le_bytes()); // MemoryFlags
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn build_win32_res(dibs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    // Required 32-byte null resource header at start of .res file.
    push_res_entry(&mut out, 0, 0, 0, &[]);

    for (idx, (_, dib)) in dibs.iter().enumerate() {
        push_res_entry(&mut out, 3 /* RT_ICON */, (idx + 1) as u16, 0x1010, dib);
    }

    let mut grp = Vec::with_capacity(6 + dibs.len() * 14);
    grp.extend_from_slice(&0u16.to_le_bytes());
    grp.extend_from_slice(&1u16.to_le_bytes());
    grp.extend_from_slice(&(dibs.len() as u16).to_le_bytes());
    for (idx, (sz, dib)) in dibs.iter().enumerate() {
        let dim = if *sz >= 256 { 0u8 } else { *sz as u8 };
        grp.push(dim);
        grp.push(dim);
        grp.push(0);
        grp.push(0);
        grp.extend_from_slice(&1u16.to_le_bytes());
        grp.extend_from_slice(&32u16.to_le_bytes());
        grp.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        grp.extend_from_slice(&((idx + 1) as u16).to_le_bytes());
    }

    push_res_entry(&mut out, 14 /* RT_GROUP_ICON */, 1, 0x1030, &grp);
    out
}
