//! Unix（Linux Wayland / X11）剪贴板读取：
//! 支持通过 `wl-paste`（Wayland）与 `xclip`（X11）读取截图图片、文件管理器复制的图片 URI 及纯文本，
//! 所有外部子进程均脱离控制终端并附带超时保护，防止无响应的剪贴板源阻塞 TUI。

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::clipboard::{
    ClipboardContent, detach_from_controlling_tty, from_image_bytes, try_load_pasted_image_paths,
};

const CLIP_TIMEOUT: Duration = Duration::from_millis(2000);

/// 读取 Linux（Wayland / X11）系统剪贴板中的图片、图片文件或文本。
///
/// 参数:
/// - `prefer_image_only`: 为 `true` 时仅读取图片或图片文件，不返回普通文本
///
/// 返回:
/// - 解析后的 `ClipboardContent`
pub fn read_clipboard(prefer_image_only: bool) -> ClipboardContent {
    // 1. Wayland 会话优先使用 wl-paste
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        if let Some(content) = read_wayland(prefer_image_only) {
            return content;
        }
    }
    // 2. X11 / XWayland 兜底使用 xclip
    if std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()) {
        if let Some(content) = read_x11(prefer_image_only) {
            return content;
        }
    }
    ClipboardContent::Empty
}

/// 通过 `wl-paste` 读取 Wayland 剪贴板。
fn read_wayland(prefer_image_only: bool) -> Option<ClipboardContent> {
    let types_raw = run_with_timeout("wl-paste", &["--list-types"])?;
    let types_text = String::from_utf8_lossy(&types_raw);
    let types = parse_mime_list(&types_text);

    // 1. 优先读取原生图片 MIME（截图工具 / 浏览器复制图片）
    if let Some(mime) = pick_image_mime(&types) {
        if let Some(bytes) = run_with_timeout("wl-paste", &["--no-newline", "--type", mime]) {
            if let Ok(img) = from_image_bytes(mime, &bytes) {
                return Some(ClipboardContent::Images(vec![img]));
            }
        }
    }

    // 2. 尝试读取文件管理器复制的 file:// URI 列表
    if has_mime(&types, "text/uri-list") {
        if let Some(uri_bytes) =
            run_with_timeout("wl-paste", &["--no-newline", "--type", "text/uri-list"])
        {
            let uri_text = String::from_utf8_lossy(&uri_bytes);
            if let Some(imgs) = load_images_from_uri_list(&uri_text) {
                return Some(ClipboardContent::Images(imgs));
            }
        }
    }

    // 3. 按需回退到纯文本（若文本本身为图片路径则自动转为图片附件）
    if !prefer_image_only {
        if let Some(bytes) = run_with_timeout("wl-paste", &["--no-newline"]) {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(imgs) = try_load_pasted_image_paths(&text) {
                return Some(ClipboardContent::Images(imgs));
            }
            if !text.is_empty() {
                return Some(ClipboardContent::Text(text));
            }
        }
    }
    None
}

/// 通过 `xclip` 读取 X11 剪贴板。
fn read_x11(prefer_image_only: bool) -> Option<ClipboardContent> {
    let types_raw = run_with_timeout("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])?;
    let types_text = String::from_utf8_lossy(&types_raw);
    let types = parse_mime_list(&types_text);

    // 1. 优先读取图片目标
    if let Some(mime) = pick_image_mime(&types) {
        if let Some(bytes) =
            run_with_timeout("xclip", &["-selection", "clipboard", "-t", mime, "-o"])
        {
            if let Ok(img) = from_image_bytes(mime, &bytes) {
                return Some(ClipboardContent::Images(vec![img]));
            }
        }
    }

    // 2. 尝试读取文件 URI 列表
    if has_mime(&types, "text/uri-list") {
        if let Some(uri_bytes) = run_with_timeout(
            "xclip",
            &["-selection", "clipboard", "-t", "text/uri-list", "-o"],
        ) {
            let uri_text = String::from_utf8_lossy(&uri_bytes);
            if let Some(imgs) = load_images_from_uri_list(&uri_text) {
                return Some(ClipboardContent::Images(imgs));
            }
        }
    }

    // 3. 回退到纯文本
    if !prefer_image_only {
        if let Some(bytes) = run_with_timeout("xclip", &["-selection", "clipboard", "-o"]) {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(imgs) = try_load_pasted_image_paths(&text) {
                return Some(ClipboardContent::Images(imgs));
            }
            if !text.is_empty() {
                return Some(ClipboardContent::Text(text));
            }
        }
    }
    None
}

/// 带超时与控制终端隔离执行外部剪贴板命令并收集标准输出。
///
/// 参数:
/// - `program`: 可执行文件名（如 `wl-paste` / `xclip`）
/// - `args`: 命令参数切片
///
/// 返回:
/// - 命令在超时内成功退出且输出非空时返回 `Some(Vec<u8>)`
fn run_with_timeout(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    detach_from_controlling_tty(&mut cmd);

    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;

    // 1. 后台线程读取完整 stdout，防止大图片超过内核管道缓冲导致子进程阻塞
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    // 2. 轮询等待退出，超时后强制终止子进程避免卡死 TUI
    let deadline = Instant::now() + CLIP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let data = reader.join().ok()?;
                return (status.success() && !data.is_empty()).then_some(data);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn parse_mime_list(raw: &str) -> Vec<&str> {
    raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

fn has_mime(types: &[&str], target: &str) -> bool {
    types.iter().any(|t| t.eq_ignore_ascii_case(target))
}

/// 按兼容优先级从剪贴板支持的 MIME 列表中挑选图片格式。
fn pick_image_mime<'a>(types: &[&'a str]) -> Option<&'a str> {
    const PRIORITY: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
        "image/bmp",
    ];
    for wanted in PRIORITY {
        if let Some(&found) = types.iter().find(|t| t.eq_ignore_ascii_case(wanted)) {
            return Some(found);
        }
    }
    None
}

/// 过滤 `text/uri-list` 中的 `#` 注释行并尝试加载为本地图片集合。
fn load_images_from_uri_list(raw: &str) -> Option<Vec<crate::clipboard::LoadedImage>> {
    let cleaned: String = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    if cleaned.is_empty() {
        return None;
    }
    try_load_pasted_image_paths(&cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_image_mime_prefers_png_over_other_formats() {
        let types = parse_mime_list("text/plain\nimage/jpeg\nimage/png\nimage/bmp\n");
        assert_eq!(pick_image_mime(&types), Some("image/png"));

        let bmp_only = parse_mime_list("text/html\nimage/bmp\n");
        assert_eq!(pick_image_mime(&bmp_only), Some("image/bmp"));
    }
}
