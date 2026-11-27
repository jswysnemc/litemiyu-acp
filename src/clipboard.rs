//! 剪贴板读写（支持图片截图/文件/文本毫秒级原生读取 + OSC 52/原生复制）与外部链接/文件打开。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;

#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub mime_type: String,
    pub base64_data: String,
    pub path: Option<PathBuf>,
    pub byte_len: usize,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
#[derive(Debug)]
pub enum ClipboardContent {
    Images(Vec<LoadedImage>),
    Text(String),
    Empty,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
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

/// 剪贴板截图落盘缓存最多保留的近期文件数，防止长期使用无限累积。
#[cfg_attr(target_os = "macos", allow(dead_code))]
const MAX_CACHED_CLIPS: usize = 50;

pub fn open_target(target: &str) {
    let cleaned = decode_file_uri_to_path(target);
    #[cfg(windows)]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", &cleaned])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(&cleaned)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach_from_controlling_tty(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        // 1. 必须通过 setsid 脱离控制终端 /dev/tty，防止默认关联为终端编辑器（如 helix/vim）时
        //    后台子进程打开 /dev/tty 与前端 TUI 争抢键盘和鼠标输入流
        let mut cmd = Command::new("xdg-open");
        cmd.arg(&cleaned)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach_from_controlling_tty(&mut cmd);
        let _ = cmd.spawn();
    }
}

/// 在 Unix 子进程启动前调用 `setsid()`，使其创建独立会话并彻底脱离父进程的控制终端 `/dev/tty`。
///
/// 参数:
/// - `cmd`: 待启动的外部命令
#[cfg(unix)]
pub(crate) fn detach_from_controlling_tty(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
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

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        crate::clipboard_unix::read_clipboard(prefer_image_only)
    }

    #[cfg(target_os = "macos")]
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
        let decoded = decode_file_uri_to_path(cleaned);
        let path = Path::new(&decoded);
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

/// 将 `file://` URI 规范化为本地文件系统路径（兼容 Windows 盘符与 Unix 根 `/` 及百分号编码）。
///
/// 参数:
/// - `raw`: 原始路径或 `file://` URI 字符串
///
/// 返回:
/// - 可供 `Path::new` 直接使用的本地路径字符串
fn decode_file_uri_to_path(raw: &str) -> String {
    let Some(rest) = raw
        .strip_prefix("file://localhost")
        .or_else(|| raw.strip_prefix("file://"))
    else {
        return raw.to_string();
    };
    let decoded = percent_decode_utf8(rest);
    // Windows 下形如 `/D:/foo/bar.png` 需去掉前导 `/`，Unix 下 `/home/foo.png` 必须保留前导 `/`
    #[cfg(windows)]
    {
        if decoded.starts_with('/')
            && decoded.len() >= 3
            && decoded.as_bytes()[2] == b':'
        {
            return decoded[1..].to_string();
        }
    }
    decoded
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 将剪贴板读取到的原始图片字节编码为 `LoadedImage`（BMP 自动转 PNG，并落盘缓存方便点击查看）。
///
/// 参数:
/// - `mime`: 原始 MIME 类型（如 `image/png`、`image/jpeg`、`image/bmp`）
/// - `raw`: 原始图片字节
///
/// 返回:
/// - 编码后的 `LoadedImage`
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn from_image_bytes(mime: &str, raw: &[u8]) -> Result<LoadedImage, String> {
    if raw.len() > 20 * 1024 * 1024 {
        return Err("剪贴板图片超过 20MB 限制".into());
    }
    let lower = mime.trim().to_ascii_lowercase();
    let (final_mime, ext, owned): (&str, &str, Option<Vec<u8>>) = match lower.as_str() {
        "image/png" => ("image/png", "png", None),
        "image/jpeg" | "image/jpg" => ("image/jpeg", "jpg", None),
        "image/webp" => ("image/webp", "webp", None),
        "image/gif" => ("image/gif", "gif", None),
        "image/bmp" => ("image/png", "png", Some(bmp_to_png(raw)?)),
        _ => return Err(format!("不支持的剪贴板图片类型：{mime}")),
    };
    let bytes = owned.as_deref().unwrap_or(raw);
    let saved = save_clip_image(bytes, ext);
    let base64_data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(LoadedImage {
        mime_type: final_mime.to_string(),
        base64_data,
        path: saved,
        byte_len: bytes.len(),
    })
}

#[cfg(windows)]
fn from_png_bytes(png_bytes: &[u8]) -> Result<LoadedImage, String> {
    from_image_bytes("image/png", png_bytes)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn save_clip_image(bytes: &[u8], ext: &str) -> Option<PathBuf> {
    let dir = crate::paths::clip_cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let seq = CLIP_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("clip_{ts}_{seq}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    prune_clip_cache(&dir, MAX_CACHED_CLIPS);
    Some(path)
}

/// 清理剪贴板图片缓存目录中超出上限的历史文件，仅保留最近的 `max_keep` 份。
///
/// 参数:
/// - `dir`: 缓存目录路径
/// - `max_keep`: 允许保留的最大文件数量
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn prune_clip_cache(dir: &Path, max_keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("clip_") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            Some((mtime, name, entry.path()))
        })
        .collect();
    if files.len() <= max_keep {
        return;
    }
    // 1. 按修改时间与文件名从新到旧排序，保留前 max_keep 个，移除其余旧缓存
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, _, old_path) in files.into_iter().skip(max_keep) {
        let _ = std::fs::remove_file(old_path);
    }
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
    let row_stride = (width as usize * bytes_per_pixel).div_ceil(4) * 4;
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
        for px in bmp[54..].as_chunks_mut::<4>().0 {
            px[0] = 0x10; // B
            px[1] = 0x20; // G
            px[2] = 0xf0; // R
            px[3] = 0x00; // all-zero alpha -> treated as opaque
        }
        let png = bmp_to_png(&bmp).expect("valid png");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[cfg(unix)]
    #[test]
    fn detached_child_cannot_open_controlling_tty() {
        use super::{Command, Stdio, detach_from_controlling_tty};

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec </dev/tty"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach_from_controlling_tty(&mut cmd);
        let status = cmd.status().expect("spawn sh");
        assert!(
            !status.success(),
            "detached child should fail to open /dev/tty"
        );
    }

    #[cfg(unix)]
    #[test]
    fn decodes_unix_file_uri_with_leading_slash_and_percent_encoding() {
        let path = super::decode_file_uri_to_path("file:///home/snemc/%E5%9B%BE%E7%89%87%201.png");
        assert_eq!(path, "/home/snemc/图片 1.png");
    }

    #[test]
    fn prunes_clip_cache_to_max_keep_and_preserves_other_files() {
        let dir = std::env::temp_dir().join(format!(
            "agy_tui_clip_prune_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        for i in 0..5 {
            std::fs::write(dir.join(format!("clip_20260101_00000{i}_{i}.png")), b"x")
                .expect("write clip");
        }
        std::fs::write(dir.join("notes.txt"), b"keep").expect("write other file");

        super::prune_clip_cache(&dir, 2);

        let mut remaining: Vec<String> = std::fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            remaining.iter().filter(|n| n.starts_with("clip_")).count(),
            2
        );
        assert!(remaining.contains(&"notes.txt".to_string()));
    }
}
