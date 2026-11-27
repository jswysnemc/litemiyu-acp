//! 外部编辑器联动（`Ctrl+G`）：优先读取 `$VISUAL` / `$EDITOR`，
//! Windows 默认回退到 `notepad.exe`，Unix 默认回退到 `vi`；
//! 兼容 Windows 11 标签页版 Notepad 快速转发退出时的保存等待。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// 解析当前生效的外部编辑器命令（`$VISUAL` > `$EDITOR` > 平台默认）。
pub fn resolve_editor_command() -> String {
    resolve_editor_from(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

fn resolve_editor_from(visual: Option<&str>, editor: Option<&str>) -> String {
    visual
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| editor.map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_string)
        .unwrap_or_else(|| default_editor().to_string())
}

fn default_editor() -> &'static str {
    #[cfg(windows)]
    {
        "notepad.exe"
    }
    #[cfg(not(windows))]
    {
        "vi"
    }
}

/// 将输入草稿写入临时 `.md` 文件并调用外部编辑器，返回编辑并规范化后的正文。
/// 调用前需由主循环暂停 TUI（退出备用屏并关闭 raw mode），返回后再恢复 TUI。
pub fn edit_in_external_editor(initial: &str) -> Result<String> {
    let editor = resolve_editor_command();
    let path = temporary_buffer_path();

    #[cfg(windows)]
    let initial_bytes = initial.replace('\n', "\r\n");
    #[cfg(not(windows))]
    let initial_bytes = initial.to_string();

    fs::write(&path, &initial_bytes)
        .with_context(|| format!("写入临时文件失败：{}", path.display()))?;

    #[cfg(windows)]
    let before = fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now());
    #[cfg(windows)]
    let launch_started = Instant::now();

    let status = build_editor_command(&editor, &path)
        .status()
        .with_context(|| format!("无法启动外部编辑器：{editor}"))?;

    if !status.success() {
        let _ = fs::remove_file(&path);
        bail!("外部编辑器异常退出 ({status})");
    }

    #[cfg(windows)]
    {
        // Windows 11 标签页版 Notepad 会将文件转发给既有窗口并立刻返回；
        // 仅在快速返回且尚未修改文件时轮询保存事件，正常阻塞退出零延迟。
        wait_for_gui_editor_save(&editor, &path, before, launch_started.elapsed());
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("读取编辑结果失败：{}", path.display()))?;
    let _ = fs::remove_file(&path);
    Ok(normalize_external_text(&raw))
}

fn temporary_buffer_path() -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("agy-tui-edit-{pid}-{nanos}.md"))
}

/// 规范化外部编辑器写回的内容：去除 UTF-8 BOM、统一换行为 `\n`，并去掉单个末尾换行。
pub fn normalize_external_text(raw: &str) -> String {
    let without_bom = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let normalized = without_bom.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .strip_suffix('\n')
        .unwrap_or(&normalized)
        .to_string()
}

fn build_editor_command(editor: &str, path: &Path) -> Command {
    #[cfg(windows)]
    {
        if let Some(cmd) = direct_windows_editor_command(editor, path) {
            return cmd;
        }
        // `.cmd` / `.bat` shim（例如 `code --wait`）经 cmd.exe 调用
        let mut cmd = Command::new("cmd.exe");
        let quoted_path = format!("\"{}\"", path.display());
        cmd.args(["/C", &format!("{editor} {quoted_path}")]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("agy-tui-editor")
            .arg(path);
        cmd
    }
}

#[cfg(any(windows, test))]
fn split_command_tokens(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in raw.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, q @ ('"' | '\'')) => quote = Some(q),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

#[cfg(windows)]
fn direct_windows_editor_command(editor: &str, path: &Path) -> Option<Command> {
    let tokens = split_command_tokens(editor);
    let head = tokens.first()?;
    let executable = if head.contains('/') || head.contains('\\') {
        let candidate = PathBuf::from(head);
        let is_exe = candidate
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        (candidate.is_file() && is_exe).then_some(candidate)
    } else if head.to_ascii_lowercase().ends_with(".exe") {
        find_exe_in_path(head)
    } else {
        find_exe_in_path(&format!("{head}.exe"))
    }?;
    let mut cmd = Command::new(executable);
    for arg in &tokens[1..] {
        cmd.arg(arg);
    }
    cmd.arg(path);
    Some(cmd)
}

#[cfg(windows)]
fn find_exe_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn wait_for_gui_editor_save(editor: &str, path: &Path, before: SystemTime, elapsed: Duration) {
    let name = editor_executable_name(editor);
    if !name.starts_with("notepad") || elapsed > Duration::from_secs(2) {
        return;
    }
    if modified_after(path, before) {
        return;
    }
    eprintln!("等待外部编辑器保存；保存后将自动返回，未保存关闭可按 Enter / Esc 返回终端…");
    let raw_enabled = crossterm::terminal::enable_raw_mode().is_ok();
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        if modified_after(path, before) {
            // 留极短缓冲让编辑器完成刷新落盘
            std::thread::sleep(Duration::from_millis(60));
            break;
        }
        if crossterm::event::poll(Duration::from_millis(200)).unwrap_or(false) {
            if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                if key.kind != crossterm::event::KeyEventKind::Release
                    && matches!(
                        key.code,
                        crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Esc
                    )
                {
                    break;
                }
            }
        }
    }
    if raw_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(windows)]
fn editor_executable_name(editor: &str) -> String {
    let tokens = split_command_tokens(editor);
    let head = tokens.first().map(String::as_str).unwrap_or_default();
    head.rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(windows)]
fn modified_after(path: &Path, before: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|modified| modified > before)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_visual_then_editor_then_platform_default() {
        assert_eq!(
            resolve_editor_from(Some("code --wait"), Some("nvim")),
            "code --wait"
        );
        assert_eq!(resolve_editor_from(Some("  "), Some("nvim")), "nvim");
        assert_eq!(resolve_editor_from(None, Some("")), default_editor());
    }

    #[test]
    fn normalizes_bom_crlf_and_single_trailing_newline() {
        assert_eq!(normalize_external_text("\u{feff}hello\r\nworld\r\n"), "hello\nworld");
        assert_eq!(normalize_external_text("single line\n"), "single line");
        assert_eq!(normalize_external_text("keep blank\n\n"), "keep blank\n");
    }

    #[test]
    fn splits_quoted_editor_command_with_args() {
        let tokens = split_command_tokens(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst -nosession"#);
        assert_eq!(
            tokens,
            vec![
                r"C:\Program Files\Notepad++\notepad++.exe",
                "-multiInst",
                "-nosession"
            ]
        );
    }
}
