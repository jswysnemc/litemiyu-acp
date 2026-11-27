//! 跨平台文件路径（Windows / Linux / POSIX / 相对路径）精准识别与分层语义着色模块。

use ratatui::style::Style;
use ratatui::text::Span;

use super::inline::Piece;
use crate::theme;

/// 识别到的单条路径或 URL 在原字符串中的字节区间及类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Url,
    Path,
}

/// 拆分后的路径各语义片段（用于分层着色：目录柔和、分隔符淡化、文件名醒目、行号强调）。
struct PathParts<'a> {
    dir: &'a str,
    file: &'a str,
    line_suffix: &'a str,
}

/// 判断给定完整 token 是否为合法的 Windows / Linux / 相对文件路径（支持尾部 `:line[:col][-end]`）。
///
/// 参数:
/// - `token`: 待检测的单个非空白片段
///
/// 返回:
/// - 符合跨平台路径特征时返回 `true`
pub fn looks_like_file_path(token: &str) -> bool {
    let core = strip_line_suffix(token).0;
    if core.len() < 2 || core.contains("://") {
        return false;
    }
    // 1. Windows 盘符路径（`C:\...` / `D:/...`）与 UNC 网络路径（`\\server\share`）
    if is_windows_drive_path(core) || is_windows_unc_path(core) {
        return true;
    }
    // 2. 显式前导路径（`~/...`、`./...`、`../...`、`.\...`、`..\...`）
    if core.starts_with("~/")
        || core.starts_with("~\\")
        || core.starts_with("./")
        || core.starts_with("../")
        || core.starts_with(".\\")
        || core.starts_with("..\\")
    {
        return core.len() >= 3;
    }
    // 3. Linux/POSIX 绝对路径（排除斜杠命令 `/help`、单段纯词与数学除法）：需至少两段或带已知后缀/隐藏名
    if core.starts_with('/') {
        let rest = &core[1..];
        if rest.is_empty() || rest.starts_with('/') {
            return false;
        }
        return rest.contains('/')
            || rest.starts_with('.')
            || has_file_extension(rest);
    }
    // 4. 相对路径（必须包含 `/` 或 `\`，避免误判普通英文词或 `true/false`、`PC/移动端`、`2/4`）
    let sep = if core.contains('\\') {
        '\\'
    } else if core.contains('/') {
        '/'
    } else {
        return false;
    };
    let segments: Vec<&str> = core.split(sep).filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return false;
    }
    // 排除纯数字比值（`2/4`、`10/20`）和无后缀的双段短词（`PC/移动端`、`展开/收起`）
    if segments
        .iter()
        .all(|s| s.chars().all(|c| c.is_ascii_digit() || c == '.'))
    {
        return false;
    }
    if segments.iter().any(|s| s.chars().any(|c| !c.is_ascii())) && !has_file_extension(segments[segments.len() - 1]) && sep == '/' && segments.len() == 2 {
        return false;
    }
    let last = segments[segments.len() - 1];
    // 含明确文件后缀、或至少 3 层路径、或常见目录前缀、或使用 Windows 反斜杠且全为合法路径字符
    has_file_extension(last)
        || is_known_filename(last)
        || (sep == '\\' && core.chars().all(is_path_char))
        || (segments.len() >= 3
            && segments
                .iter()
                .all(|s| !s.is_empty() && s.chars().all(is_segment_char)))
        || (matches!(
            segments[0],
            "src" | "tests" | "test" | "app" | "lib" | "bin" | "cmd" | "pkg" | "internal"
                | "docs" | "doc" | "scripts" | "config" | "resources" | "assets" | "target"
                | "packages" | "crates" | "modules" | "components" | "features"
        ) && segments
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(is_segment_char)))
}

/// 将路径按「同色系柔和目录 / 清晰分隔符 / 加粗文件名 / 暖色行号」克制分层着色为 `Piece` 列表。
///
/// 参数:
/// - `path`: 完整文件路径字符串（可含 `:45-51` 或 `#L10` 后缀）
/// - `link`: 可选点击跳转目标（传入 `Some` 时所有片段绑定同一链接）
///
/// 返回:
/// - 分层着色后的行内 `Piece` 集合
pub fn style_path_pieces(path: &str, link: Option<&str>) -> Vec<Piece> {
    let parts = split_path_parts(path);
    // 1. 保持路径整体连贯：盘符/目录用柔和路径色，分隔符用清晰 dim（避免透明背景下隐身），文件名用同色系加粗
    let dir_style = theme::md_url();
    let sep_style = theme::dim();
    let file_style = theme::md_link();
    let line_style = theme::code_number();

    let mut out = Vec::new();
    let push = |out: &mut Vec<Piece>, text: &str, style: Style| {
        if text.is_empty() {
            return;
        }
        let piece = match link {
            Some(url) => Piece::linked(text, style, url),
            None => Piece::new(text, style),
        };
        out.push(piece);
    };

    // 2. 目录与分隔符（保持完整连续感，不额外拆分多种撞色）
    if !parts.dir.is_empty() {
        let mut start = 0usize;
        for (idx, ch) in parts.dir.char_indices() {
            if ch == '/' || ch == '\\' {
                if idx > start {
                    push(&mut out, &parts.dir[start..idx], dir_style);
                }
                push(&mut out, &parts.dir[idx..idx + 1], sep_style);
                start = idx + 1;
            }
        }
        if start < parts.dir.len() {
            push(&mut out, &parts.dir[start..], dir_style);
        }
    }

    // 3. 尾部文件名主体加粗突出
    if !parts.file.is_empty() {
        push(&mut out, parts.file, file_style);
    }

    // 4. 尾部行号/列号区间（如 `:45-51`、`#L12`）：冒号清晰可见，数字采用暖色强调
    if !parts.line_suffix.is_empty() {
        if let Some(rest) = parts.line_suffix.strip_prefix(':') {
            push(&mut out, ":", sep_style);
            push(&mut out, rest, line_style);
        } else {
            push(&mut out, parts.line_suffix, line_style);
        }
    }

    out
}

/// 将文件路径分层着色为 ratatui `Span` 列表（供工具路径行、Diff 标题等直接使用）。
///
/// 参数:
/// - `path`: 待渲染路径字符串
///
/// 返回:
/// - 分层着色后的 `Span<'static>` 集合
pub fn style_path_spans(path: &str) -> Vec<Span<'static>> {
    style_path_pieces(path, None)
        .into_iter()
        .map(|p| Span::styled(p.text, p.style))
        .collect()
}

/// 扫描一段自然语言/终端输出文本中的裸 URL 与 Windows/Linux 文件路径区间（不重叠，按起始位置排序）。
///
/// 参数:
/// - `text`: 原始单行或段落文本
///
/// 返回:
/// - `(起始字节, 结束字节, SpanKind)` 有序列表
pub fn find_urls_and_paths(text: &str) -> Vec<(usize, usize, SpanKind)> {
    let mut found = Vec::new();
    let urls = find_bare_urls(text);
    for (s, e) in &urls {
        found.push((*s, *e, SpanKind::Url));
    }

    // 在非 URL 区间中按边界扫描候选路径 token
    let mut cursor = 0usize;
    let url_iter = urls.iter().copied().chain(std::iter::once((text.len(), text.len())));
    for (u_start, u_end) in url_iter {
        if u_start > cursor {
            scan_paths_in_slice(&text[cursor..u_start], cursor, &mut found);
        }
        cursor = u_end;
    }

    found.sort_by_key(|(s, _, _)| *s);
    found
}

/// 去掉路径末尾 `:line[:col][-end]` 或 `#L...` 后缀，返回 `(纯路径, 行号后缀)`，供打开本地文件与着色复用。
pub fn strip_line_suffix(token: &str) -> (&str, &str) {
    if let Some(hash_idx) = token.rfind("#L")
        && hash_idx > 0
        && token[hash_idx + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    {
        return (&token[..hash_idx], &token[hash_idx..]);
    }
    // 从尾部剥离 1~2 段 `:digits[-digits]`，注意跳过 Windows 盘符 `C:`（idx == 1）
    let mut end = token.len();
    for _ in 0..2 {
        let Some(colon) = token[..end].rfind(':') else {
            break;
        };
        if colon <= 1 {
            break;
        }
        let tail = &token[colon + 1..end];
        if !tail.is_empty()
            && tail
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == 'L')
            && tail.chars().any(|c| c.is_ascii_digit())
        {
            end = colon;
        } else {
            break;
        }
    }
    (&token[..end], &token[end..])
}

fn split_path_parts(path: &str) -> PathParts<'_> {
    let (core, line_suffix) = strip_line_suffix(path);
    if let Some(sep_idx) = core.rfind(['/', '\\']) {
        let split_at = sep_idx + 1;
        PathParts {
            dir: &core[..split_at],
            file: &core[split_at..],
            line_suffix,
        }
    } else {
        PathParts {
            dir: "",
            file: core,
            line_suffix,
        }
    }
}

fn is_windows_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/')
}

fn is_windows_unc_path(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("\\\\") {
        let mut parts = rest.split('\\').filter(|p| !p.is_empty());
        parts.next().is_some() && parts.next().is_some()
    } else {
        false
    }
}

fn has_file_extension(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !ext.is_empty()
        && ext.len() <= 10
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
}

fn is_known_filename(name: &str) -> bool {
    matches!(
        name,
        "Makefile"
            | "Dockerfile"
            | "Justfile"
            | "Cargo.toml"
            | "Cargo.lock"
            | "LICENSE"
            | "README"
            | ".gitignore"
            | ".env"
            | ".bashrc"
            | ".zshrc"
    )
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '/' | '\\' | '.' | '_' | '-' | ':' | '~' | '@' | '+' | '#'
        )
}

fn is_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '+')
}

fn scan_paths_in_slice(slice: &str, base_offset: usize, out: &mut Vec<(usize, usize, SpanKind)>) {
    const DELIMS: &[char] = &[
        ' ', '\t', '\n', '\r', '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '`', ',', ';',
        '（', '）', '【', '】', '「', '」', '《', '》', '，', '。', '；', '！', '？', '、', '：',
    ];
    let mut i = 0usize;
    while i < slice.len() {
        let rest = &slice[i..];
        let ch = rest.chars().next().unwrap();
        if DELIMS.contains(&ch) {
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        while i < slice.len() {
            let c = slice[i..].chars().next().unwrap();
            if DELIMS.contains(&c) {
                break;
            }
            i += c.len_utf8();
        }
        let mut tok_start = start;
        let mut tok_end = i;
        // 剥离尾部普通标点（如句末 `.` `:` `?`）
        while tok_end > tok_start {
            let last = slice[tok_start..tok_end].chars().next_back().unwrap();
            if matches!(last, '.' | ':' | '!' | '?' | ',') {
                tok_end -= last.len_utf8();
            } else {
                break;
            }
        }
        // 处理中文或英文前缀紧贴路径的情况（如 `路径/home/a.rs` 不拆；跳过前导无意义符号如 `*`）
        while tok_start < tok_end {
            let first = slice[tok_start..tok_end].chars().next().unwrap();
            if matches!(first, '*' | '=' | '|') {
                tok_start += first.len_utf8();
            } else {
                break;
            }
        }
        if tok_end > tok_start {
            let candidate = &slice[tok_start..tok_end];
            if looks_like_file_path(candidate) {
                out.push((base_offset + tok_start, base_offset + tok_end, SpanKind::Path));
            }
        }
    }
}

/// 扫描文本中的裸 `https://` / `http://` / `file://` URL 区间。
pub fn find_bare_urls(text: &str) -> Vec<(usize, usize)> {
    const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];
    const TAIL: &str = ".,;:!?'\"。，；：！？、";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some((start, scheme)) = SCHEMES
        .iter()
        .filter_map(|s| text[from..].find(s).map(|i| (from + i, *s)))
        .min_by_key(|(i, _)| *i)
    {
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        let body = text[start..]
            .find(|c: char| {
                !(c.is_ascii_graphic()
                    && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^'))
            })
            .map_or(text.len(), |i| start + i);
        let raw = &text[start..body];
        let mut end_rel = raw.len();
        while let Some(last) = raw[..end_rel].chars().next_back() {
            let opener = match last {
                ')' => Some('('),
                ']' => Some('['),
                _ => None,
            };
            if let Some(open) = opener {
                let slice = &raw[..end_rel];
                if slice.matches(last).count() <= slice.matches(open).count() {
                    break;
                }
            } else if !TAIL.contains(last) {
                break;
            }
            end_rel -= last.len_utf8();
        }
        let end = start + end_rel;
        if !glued && end > start + scheme.len() {
            found.push((start, end));
        }
        from = body.max(start + scheme.len());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, ThemePreset};

    #[test]
    fn recognizes_windows_and_linux_paths_without_false_positives() {
        assert!(looks_like_file_path(r"C:\Users\snemc\project\main.rs"));
        assert!(looks_like_file_path(r"D:/Code/Systemui-dev/Foo.kt:45-51"));
        assert!(looks_like_file_path(r"\\fileserver\share\docs\spec.md"));
        assert!(looks_like_file_path("/home/snemc/workspace/agy-tui/src/main.rs"));
        assert!(looks_like_file_path("~/.config/agy/settings.json"));
        assert!(looks_like_file_path("./src/ui/overlay.rs:120"));
        assert!(looks_like_file_path("src/ui/overlay.rs"));

        assert!(!looks_like_file_path("/help"));
        assert!(!looks_like_file_path("2/4"));
        assert!(!looks_like_file_path("PC/移动端"));
        assert!(!looks_like_file_path("https://example.com/a/b"));
    }

    #[test]
    fn styles_windows_and_linux_paths_with_distinct_layers() {
        theme::init(ThemePreset::MiyuLavender, true);
        let win = style_path_pieces(r"D:\Code\app\main.rs:42", None);
        assert!(win.iter().any(|p| p.text == "D:" && p.style == theme::md_url()));
        assert!(win.iter().any(|p| p.text == "\\" && p.style == theme::dim()));
        assert!(win.iter().any(|p| p.text == "main.rs" && p.style == theme::md_link()));
        assert!(win.iter().any(|p| p.text == "42" && p.style == theme::code_number()));

        let lin = style_path_pieces("/home/snemc/src/lib.rs:10-20", None);
        assert!(lin.iter().any(|p| p.text == "/" && p.style == theme::dim()));
        assert!(lin.iter().any(|p| p.text == "lib.rs" && p.style == theme::md_link()));
        assert!(lin.iter().any(|p| p.text == "10-20" && p.style == theme::code_number()));
    }
}
