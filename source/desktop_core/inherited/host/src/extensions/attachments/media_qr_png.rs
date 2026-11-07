use std::path::{Path, PathBuf};
use std::io::Write;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use uuid::Uuid;

pub const COMPACT_MARGIN_MODULES: i32 = 1;
pub const TERMINAL_BLACK_ON_WHITE: &str = "\x1b[47m\x1b[30m";
pub const TERMINAL_RESET: &str = "\x1b[0m";
pub const QR_PNG_DATA_URL_PREFIX: &str = "data:image/png;base64,";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPngRenderOptions {
    pub scale: Option<u32>,
    pub margin_modules: Option<u32>,
}

impl Default for QrPngRenderOptions {
    fn default() -> Self {
        Self {
            scale: Some(6),
            margin_modules: Some(4),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPngTempFileOptions {
    pub tmp_root: PathBuf,
    pub dir_prefix: String,
    pub file_name: Option<String>,
    pub render_options: QrPngRenderOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPngTempFile {
    pub file_path: String,
    pub dir_path: String,
    pub media_local_roots: Vec<String>,
}

pub fn resolve_qr_png_integer_option(name: &str, input: f64, min: i64, max: i64) -> Result<i64, String> {
    if !input.is_finite() {
        return Err(format!("{name} must be a finite number."));
    }
    let value = input.floor() as i64;
    if value < min || value > max {
        return Err(format!("{name} must be between {min} and {max}."));
    }
    Ok(value)
}

pub fn resolve_qr_temp_path_segment(name: &str, value: &str) -> Result<String, String> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(format!("{name} must be a non-empty filename segment."));
    }
    let path = Path::new(value);
    if path.file_name().and_then(|s| s.to_str()) != Some(value) {
        return Err(format!("{name} must be a non-empty filename segment."));
    }
    Ok(value.to_string())
}

fn read_module(modules: &[qrcode::Color], size: usize, x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
        return false;
    }
    matches!(modules[(y as usize) * size + (x as usize)], qrcode::Color::Dark)
}

pub fn render_qr_terminal(input: &str, small: bool) -> Result<String, String> {
    let code = qrcode::QrCode::new(input.as_bytes()).map_err(|e| format!("Failed to generate QR code: {e}"))?;
    let size = code.width();
    let modules = code.to_colors();

    if small {
        let mut lines = Vec::new();
        let margin = COMPACT_MARGIN_MODULES;
        let mut y = -margin;
        while y < size as i32 + margin {
            let mut line = String::from(TERMINAL_BLACK_ON_WHITE);
            for x in -margin..(size as i32 + margin) {
                let top = read_module(&modules, size, x, y);
                let bottom = read_module(&modules, size, x, y + 1);
                let ch = match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                };
                line.push(ch);
            }
            line.push_str(TERMINAL_RESET);
            lines.push(line);
            y += 2;
        }
        Ok(lines.join("\n"))
    } else {
        let margin = 4i32;
        let mut lines = Vec::new();
        for y in -margin..(size as i32 + margin) {
            let mut line = String::new();
            for x in -margin..(size as i32 + margin) {
                let dark = read_module(&modules, size, x, y);
                if dark {
                    line.push_str("██");
                } else {
                    line.push_str("  ");
                }
            }
            lines.push(line);
        }
        Ok(lines.join("\n"))
    }
}

fn crc32(type_tag: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(type_tag);
    crc.update(data);
    crc.sum()
}

pub fn png_chunk_parts(type_tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let len = data.len() as u32;
    let mut chunk = Vec::with_capacity(12 + data.len());
    chunk.extend_from_slice(&len.to_be_bytes());
    chunk.extend_from_slice(type_tag);
    chunk.extend_from_slice(data);
    let checksum = crc32(type_tag, data);
    chunk.extend_from_slice(&checksum.to_be_bytes());
    chunk
}

pub fn fill_pixel(
    buf: &mut [u8],
    x: i32,
    y: i32,
    width: usize,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    if x < 0 || y < 0 || x >= width as i32 {
        return;
    }
    let idx = (y * (width as i32) + x) * 4;
    if idx < 0 || (idx + 3) as usize >= buf.len() {
        return;
    }
    let idx = idx as usize;
    buf[idx] = r;
    buf[idx + 1] = g;
    buf[idx + 2] = b;
    buf[idx + 3] = a;
}

pub fn encode_png(buffer: &[u8], width: u32, height: u32, channels: u8) -> Result<Vec<u8>, String> {
    if channels != 3 && channels != 4 {
        return Err("Only 3 (RGB) or 4 (RGBA) channels supported".to_string());
    }
    let stride = (width as usize) * (channels as usize);
    let expected_len = stride * (height as usize);
    if buffer.len() != expected_len {
        return Err(format!("Buffer length {} does not match expected {}", buffer.len(), expected_len));
    }

    let mut raw = vec![0u8; (stride + 1) * (height as usize)];
    for row in 0..(height as usize) {
        let raw_offset = row * (stride + 1);
        raw[raw_offset] = 0; // Filter byte 0 (None)
        let buf_offset = row * stride;
        raw[raw_offset + 1..raw_offset + 1 + stride].copy_from_slice(&buffer[buf_offset..buf_offset + stride]);
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).map_err(|e| format!("Failed to compress raw scanlines: {e}"))?;
    let compressed = encoder.finish().map_err(|e| format!("Failed to finish compression: {e}"))?;

    let signature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = if channels == 4 { 6 } else { 2 }; // color type 2=RGB, 6=RGBA
    ihdr[10] = 0; // compression
    ihdr[11] = 0; // filter
    ihdr[12] = 0; // interlace

    let mut out = Vec::new();
    out.extend_from_slice(&signature);
    out.extend_from_slice(&png_chunk_parts(b"IHDR", &ihdr));
    out.extend_from_slice(&png_chunk_parts(b"IDAT", &compressed));
    out.extend_from_slice(&png_chunk_parts(b"IEND", &[]));
    Ok(out)
}

pub fn encode_png_rgb(buffer: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    encode_png(buffer, width, height, 3)
}

pub fn encode_png_rgba(buffer: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    encode_png(buffer, width, height, 4)
}

pub fn render_qr_png_buffer(input: &str, opts: &QrPngRenderOptions) -> Result<Vec<u8>, String> {
    let scale = resolve_qr_png_integer_option("scale", opts.scale.unwrap_or(6) as f64, 1, 12)? as u32;
    let margin = resolve_qr_png_integer_option("marginModules", opts.margin_modules.unwrap_or(4) as f64, 0, 16)? as i32;

    let code = qrcode::QrCode::new(input.as_bytes()).map_err(|e| format!("Failed to generate QR code: {e}"))?;
    let size = code.width();
    let modules = code.to_colors();

    let total_modules = (size as i32) + 2 * margin;
    let dim = (total_modules as u32) * scale;
    let mut rgb_buf = vec![255u8; (dim as usize) * (dim as usize) * 3];

    for my in 0..total_modules {
        let qy = my - margin;
        for mx in 0..total_modules {
            let qx = mx - margin;
            let dark = read_module(&modules, size, qx, qy);
            let color = if dark { 0u8 } else { 255u8 };

            for py in 0..scale {
                let y = (my as u32) * scale + py;
                for px in 0..scale {
                    let x = (mx as u32) * scale + px;
                    let idx = ((y * dim + x) * 3) as usize;
                    rgb_buf[idx] = color;
                    rgb_buf[idx + 1] = color;
                    rgb_buf[idx + 2] = color;
                }
            }
        }
    }

    encode_png_rgb(&rgb_buf, dim, dim)
}

pub fn render_qr_png_base64(input: &str, opts: &QrPngRenderOptions) -> Result<String, String> {
    let buf = render_qr_png_buffer(input, opts)?;
    use base64::Engine;
    Ok(base64::prelude::BASE64_STANDARD.encode(&buf))
}

pub fn render_qr_png_data_url(input: &str, opts: &QrPngRenderOptions) -> Result<String, String> {
    let b64 = render_qr_png_base64(input, opts)?;
    Ok(format!("{}{}", QR_PNG_DATA_URL_PREFIX, b64))
}

pub fn write_qr_png_temp_file(
    input: &str,
    opts: &QrPngTempFileOptions,
) -> Result<QrPngTempFile, String> {
    let dir_prefix = resolve_qr_temp_path_segment("dirPrefix", &opts.dir_prefix)?;
    let file_name = resolve_qr_temp_path_segment("fileName", opts.file_name.as_deref().unwrap_or("qr.png"))?;
    let png = render_qr_png_buffer(input, &opts.render_options)?;

    let dir_name = format!("{}_{}", dir_prefix, Uuid::new_v4().simple());
    let dir_path = opts.tmp_root.join(dir_name);
    std::fs::create_dir_all(&dir_path).map_err(|e| format!("Failed to create temp dir {}: {e}", dir_path.display()))?;

    let file_path = dir_path.join(&file_name);
    std::fs::write(&file_path, png).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dir_path);
        format!("Failed to write PNG file {}: {e}", file_path.display())
    })?;

    let dir_str = dir_path.to_string_lossy().to_string();
    Ok(QrPngTempFile {
        file_path: file_path.to_string_lossy().to_string(),
        dir_path: dir_str.clone(),
        media_local_roots: vec![dir_str],
    })
}

pub fn convert_bmp_to_png(buffer: &[u8]) -> Result<Vec<u8>, String> {
    if buffer.len() < 54 {
        return Err("BMP buffer too small to contain valid header".to_string());
    }
    if buffer[0] != 0x42 || buffer[1] != 0x4D {
        return Err("Invalid BMP signature (expected 'BM')".to_string());
    }

    let offset = u32::from_le_bytes(buffer[10..14].try_into().unwrap()) as usize;
    let dib_header_size = u32::from_le_bytes(buffer[14..18].try_into().unwrap());
    if dib_header_size < 40 {
        return Err(format!("Unsupported DIB header size {}", dib_header_size));
    }

    let width = i32::from_le_bytes(buffer[18..22].try_into().unwrap());
    let height = i32::from_le_bytes(buffer[22..26].try_into().unwrap());
    if width <= 0 || height == 0 {
        return Err(format!("Invalid BMP dimensions: {}x{}", width, height));
    }

    let planes = u16::from_le_bytes(buffer[26..28].try_into().unwrap());
    if planes != 1 {
        return Err(format!("Unsupported planes count {}", planes));
    }

    let bpp = u16::from_le_bytes(buffer[28..30].try_into().unwrap());
    if bpp != 24 && bpp != 32 {
        return Err(format!("Unsupported BMP bit depth {} (only 24 and 32 supported)", bpp));
    }

    let compression = u32::from_le_bytes(buffer[30..34].try_into().unwrap());
    if compression != 0 {
        return Err(format!("Unsupported BMP compression {} (only uncompressed BI_RGB=0 supported)", compression));
    }

    let width_u = width as usize;
    let abs_height = height.unsigned_abs() as usize;
    let top_down = height < 0;

    let row_stride = if bpp == 24 {
        (width_u * 3 + 3) & !3
    } else {
        width_u * 4
    };

    if buffer.len() < offset + row_stride * abs_height {
        return Err("BMP buffer truncated before all scanlines could be read".to_string());
    }

    if bpp == 24 {
        let mut rgb_buf = vec![0u8; width_u * abs_height * 3];
        for y in 0..abs_height {
            let src_y = if top_down { y } else { abs_height - 1 - y };
            let src_row = offset + src_y * row_stride;
            let dst_row = y * width_u * 3;
            for x in 0..width_u {
                let src_px = src_row + x * 3;
                let dst_px = dst_row + x * 3;
                let b = buffer[src_px];
                let g = buffer[src_px + 1];
                let r = buffer[src_px + 2];
                rgb_buf[dst_px] = r;
                rgb_buf[dst_px + 1] = g;
                rgb_buf[dst_px + 2] = b;
            }
        }
        encode_png_rgb(&rgb_buf, width as u32, abs_height as u32)
    } else {
        let mut rgba_buf = vec![0u8; width_u * abs_height * 4];
        for y in 0..abs_height {
            let src_y = if top_down { y } else { abs_height - 1 - y };
            let src_row = offset + src_y * row_stride;
            let dst_row = y * width_u * 4;
            for x in 0..width_u {
                let src_px = src_row + x * 4;
                let dst_px = dst_row + x * 4;
                let b = buffer[src_px];
                let g = buffer[src_px + 1];
                let r = buffer[src_px + 2];
                let a = buffer[src_px + 3];
                rgba_buf[dst_px] = r;
                rgba_buf[dst_px + 1] = g;
                rgba_buf[dst_px + 2] = b;
                rgba_buf[dst_px + 3] = a;
            }
        }
        encode_png_rgba(&rgba_buf, width as u32, abs_height as u32)
    }
}
