//! 剪贴板读写（支持图片截图/文件/文本毫秒级原生读取 + OSC 52/原生复制）与外部链接/文件打开。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;

#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub mime_type: String,
    pub base64_data: String,
    pub path: Option<PathBuf>,
    pub byte_len: usize,
}

#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum ClipboardContent {
    Images(Vec<LoadedImage>),
    Text(String),
    Empty,
}

#[cfg(windows)]
static CLIP_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn copy_text(text: &str) {
    // 1. OSC 52（跨 SSH / Kitty / Pebrel 通用）
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{b64}\x07");
    let _ = stdout.flush();

    // 2. Windows 原生剪贴板（毫秒级 UTF-16 无乱码）
    #[cfg(windows)]
    {
        let _ = clipboard_win::set_clipboard_string(text);
    }
}

pub fn open_target(target: &str) {
    let cleaned = target
        .strip_prefix("file:///")
        .or_else(|| target.strip_prefix("file://"))
        .unwrap_or(target);
    #[cfg(windows)]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", cleaned])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open")
            .arg(cleaned)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open")
            .arg(cleaned)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// 尝试读取系统剪贴板中的图片（截图或复制的图片文件），若无图片则返回文本或 Empty。
pub fn read_clipboard(prefer_image_only: bool) -> ClipboardContent {
    #[cfg(windows)]
    {
        // 1. 优先尝试注册格式 "PNG"（浏览器/Snipaste/微信/截图工具直接提供 PNG 字节）
        if let Some(png_fmt) = clipboard_win::register_format("PNG") {
            if let Ok(raw) = clipboard_win::get_clipboard::<Vec<u8>, _>(
                clipboard_win::formats::RawData(png_fmt.get()),
            ) {
                if raw.starts_with(b"\x89PNG\r\n\x1a\n") {
                    if let Ok(img) = from_png_bytes(&raw) {
                        return ClipboardContent::Images(vec![img]);
                    }
                }
            }
        }

        // 2. 尝试读取 Windows 剪贴板 CF_DIB 位图（Win+Shift+S / 画图 / 截图工具）
        if let Ok(bmp_bytes) = clipboard_win::get_clipboard::<Vec<u8>, _>(clipboard_win::formats::Bitmap) {
            if let Ok(png_bytes) = bmp_to_png(&bmp_bytes) {
                if let Ok(img) = from_png_bytes(&png_bytes) {
                    return ClipboardContent::Images(vec![img]);
                }
            }
            // 非常规位图压缩格式（如 16-bit / 调色板）走 PowerShell System.Windows.Forms 兜底
            if let Some(img) = read_clipboard_image_powershell() {
                return ClipboardContent::Images(vec![img]);
            }
        }

        // 3. 尝试读取资源管理器 Ctrl+C 复制的文件列表（CF_HDROP）
        if let Ok(files) = clipboard_win::get_clipboard::<Vec<String>, _>(clipboard_win::formats::FileList) {
            let imgs: Vec<LoadedImage> = files
                .iter()
                .filter_map(|p| load_image_file(Path::new(p)).ok())
                .collect();
            if !imgs.is_empty() {
                return ClipboardContent::Images(imgs);
            }
            if !prefer_image_only && !files.is_empty() {
                return ClipboardContent::Text(files.join(" "));
            }
        }

        // 4. 读取纯文本（若是指向真实存在的本地图片路径，自动转为图片附件）
        if !prefer_image_only {
            if let Ok(text) = clipboard_win::get_clipboard_string() {
                if let Some(imgs) = try_load_pasted_image_paths(&text) {
                    return ClipboardContent::Images(imgs);
                }
                if !text.is_empty() {
                    return ClipboardContent::Text(text);
                }
            }
        }

        ClipboardContent::Empty
    }

    #[cfg(not(windows))]
    {
        let _ = prefer_image_only;
        ClipboardContent::Empty
    }
}

/// 仅快速读取系统剪贴板纯文本（供 Windows 无括号多行粘贴首行比对，不触发图片转换）。
pub fn read_clipboard_text() -> Option<String> {
    #[cfg(windows)]
    {
        clipboard_win::get_clipboard_string()
            .ok()
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 检查一段粘贴/拖拽文本是否全部由本地存在的图片路径组成（支持引号包裹与多行拖拽）。
pub fn try_load_pasted_image_paths(raw: &str) -> Option<Vec<LoadedImage>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return None;
    }
    let candidates: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if candidates.is_empty() || candidates.len() > 8 {
        return None;
    }
    let mut loaded = Vec::new();
    for line in candidates {
        let cleaned = strip_quotes(line);
        let path_str = cleaned
            .strip_prefix("file:///")
            .or_else(|| cleaned.strip_prefix("file://"))
            .unwrap_or(cleaned);
        let path = Path::new(path_str);
        if !is_image_extension(path) || !path.is_file() {
            return None;
        }
        let img = load_image_file(path).ok()?;
        loaded.push(img);
    }
    (!loaded.is_empty()).then_some(loaded)
}

/// 从磁盘加载图片文件（支持 png/jpg/jpeg/webp/gif，bmp 自动转 png）。
pub fn load_image_file(path: &Path) -> Result<LoadedImage, String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let raw = std::fs::read(path).map_err(|e| format!("读取图片失败：{e}"))?;
    if raw.len() > 20 * 1024 * 1024 {
        return Err("图片超过 20MB 限制".into());
    }

    let (mime_type, bytes) = match ext.as_str() {
        "png" => ("image/png".to_string(), raw),
        "jpg" | "jpeg" => ("image/jpeg".to_string(), raw),
        "webp" => ("image/webp".to_string(), raw),
        "gif" => ("image/gif".to_string(), raw),
        "bmp" => {
            let converted = bmp_to_png(&raw)?;
            ("image/png".to_string(), converted)
        }
        _ => return Err(format!("不支持的图片格式：.{ext}")),
    };

    let byte_len = bytes.len();
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(LoadedImage {
        mime_type,
        base64_data,
        path: Some(path.to_path_buf()),
        byte_len,
    })
}

#[cfg(windows)]
fn from_png_bytes(png_bytes: &[u8]) -> Result<LoadedImage, String> {
    if png_bytes.len() > 20 * 1024 * 1024 {
        return Err("剪贴板图片超过 20MB 限制".into());
    }
    let saved = save_clip_png(png_bytes);
    let base64_data = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    Ok(LoadedImage {
        mime_type: "image/png".to_string(),
        base64_data,
        path: saved,
        byte_len: png_bytes.len(),
    })
}

#[cfg(windows)]
fn save_clip_png(png_bytes: &[u8]) -> Option<PathBuf> {
    let dir = crate::paths::clip_cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let seq = CLIP_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("clip_{ts}_{seq}.png"));
    std::fs::write(&path, png_bytes).ok()?;
    Some(path)
}

/// 将 24-bit / 32-bit 无压缩（BI_RGB / BI_BITFIELDS）BMP 字节转为标准 PNG。
pub fn bmp_to_png(bmp: &[u8]) -> Result<Vec<u8>, String> {
    if bmp.len() < 54 || &bmp[0..2] != b"BM" {
        return Err("不是有效的 BMP 数据".into());
    }
    let pixel_offset = u32::from_le_bytes([bmp[10], bmp[11], bmp[12], bmp[13]]) as usize;
    let dib_size = u32::from_le_bytes([bmp[14], bmp[15], bmp[16], bmp[17]]) as usize;
    if dib_size < 40 || bmp.len() < 14 + dib_size {
        return Err("不支持的 DIB 头".into());
    }
    let width_i32 = i32::from_le_bytes([bmp[18], bmp[19], bmp[20], bmp[21]]);
    let height_i32 = i32::from_le_bytes([bmp[22], bmp[23], bmp[24], bmp[25]]);
    let bit_count = u16::from_le_bytes([bmp[28], bmp[29]]);
    let compression = u32::from_le_bytes([bmp[30], bmp[31], bmp[32], bmp[33]]);

    if width_i32 <= 0 || height_i32 == 0 {
        return Err("BMP 尺寸无效".into());
    }
    let width = width_i32 as u32;
    let top_down = height_i32 < 0;
    let height = height_i32.unsigned_abs();
    if width > 16384 || height > 16384 {
        return Err("BMP 尺寸过大".into());
    }

    // BI_RGB (0) 或 BI_BITFIELDS (3) 常见于 Windows 剪贴板 24/32-bit 截图
    if (bit_count != 24 && bit_count != 32) || (compression != 0 && compression != 3) {
        return Err(format!("非常规 BMP 格式: {bit_count}bpp comp={compression}"));
    }

    let bytes_per_pixel = (bit_count / 8) as usize;
    let row_stride = ((width as usize * bytes_per_pixel + 3) / 4) * 4;
    let needed = pixel_offset
        .checked_add(row_stride.saturating_mul(height as usize))
        .ok_or_else(|| "BMP 大小溢出".to_string())?;
    if bmp.len() < needed {
        return Err("BMP 像素数据不完整".into());
    }

    // 32bpp 截图在部分 Windows 应用里 alpha 全为 0，若检测全 0 则视作不透明 255
    let mut all_alpha_zero = bit_count == 32;
    if all_alpha_zero {
        'check: for y in 0..height as usize {
            let row = &bmp[pixel_offset + y * row_stride..];
            for x in 0..width as usize {
                if row[x * 4 + 3] != 0 {
                    all_alpha_zero = false;
                    break 'check;
                }
            }
        }
    }

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for y in 0..height as usize {
        let src_y = if top_down { y } else { (height as usize) - 1 - y };
        let src_row = &bmp[pixel_offset + src_y * row_stride..];
        let dst_row = &mut rgba[y * (width as usize) * 4..];
        for x in 0..width as usize {
            let s = x * bytes_per_pixel;
            let d = x * 4;
            dst_row[d] = src_row[s + 2]; // R
            dst_row[d + 1] = src_row[s + 1]; // G
            dst_row[d + 2] = src_row[s]; // B
            dst_row[d + 3] = if bit_count == 32 && !all_alpha_zero {
                src_row[s + 3]
            } else {
                255
            };
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG 编码失败：{e}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| format!("PNG 写入失败：{e}"))?;
    }
    Ok(out)
}

#[cfg(windows)]
fn read_clipboard_image_powershell() -> Option<LoadedImage> {
    let dir = crate::paths::clip_cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let seq = CLIP_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let out_path = dir.join(format!("clip_{ts}_{seq}.png"));
    let escaped = out_path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
         $img = [System.Windows.Forms.Clipboard]::GetImage(); \
         if ($img -ne $null) {{ $img.Save('{escaped}', [System.Drawing.Imaging.ImageFormat]::Png); $img.Dispose(); exit 0 }} else {{ exit 1 }}"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() || !out_path.is_file() {
        return None;
    }
    load_image_file(&out_path).ok()
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::bmp_to_png;

    #[test]
    fn converts_32bit_bmp_to_valid_png() {
        // 构造一个 2x2 32-bit BI_RGB BMP
        let mut bmp = vec![0u8; 54 + 2 * 2 * 4];
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[10] = 54;
        bmp[14] = 40;
        bmp[18..22].copy_from_slice(&2i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&32u16.to_le_bytes());
        for px in bmp[54..].chunks_exact_mut(4) {
            px[0] = 0x10; // B
            px[1] = 0x20; // G
            px[2] = 0xf0; // R
            px[3] = 0x00; // all-zero alpha -> treated as opaque
        }
        let png = bmp_to_png(&bmp).expect("valid png");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
