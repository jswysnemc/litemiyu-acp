//! Shell 命令语法高亮截断与通用工具明细行构建辅助。

use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;
use crate::markdown;
use crate::theme;
use crate::transcript::FoldSection;

use super::fold::{self, ActionBarOptions};
use super::{RenderRow, RowAction, RowCopyMeta};

pub(super) fn extract_command_text(
    name: &str,
    kind: &str,
    title: &str,
    raw_input: Option<&Value>,
) -> Option<String> {
    if let Some(inp) = raw_input
        && let Some(cmd) = inp["CommandLine"]
            .as_str()
            .or_else(|| inp["command"].as_str())
            .or_else(|| inp["cmd"].as_str())
    {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let is_exec = kind == "execute"
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "run_command" | "execute" | "shell" | "bash" | "powershell" | "terminal"
        );
    if is_exec {
        let cleaned = title.trim().strip_prefix("$ ").unwrap_or(title.trim());
        if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case(name) {
            return Some(cleaned.to_string());
        }
    }
    None
}

/// 将单行 Shell 命令按指定最大显示宽度截断并保留语法着色。
pub(super) fn clip_highlighted_shell(cmd: &str, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let pieces = markdown::code::highlight_shell_line(cmd, None);
    let total_w: usize = pieces.iter().map(|p| p.text.width()).sum();
    if total_w <= max_width {
        return pieces
            .into_iter()
            .map(|p| Span::styled(p.text, p.style))
            .collect();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = Vec::new();
    'outer: for p in pieces {
        let mut buf = String::new();
        for g in p.text.graphemes(true) {
            let w = g.width();
            if used + w > budget {
                if !buf.is_empty() {
                    out.push(Span::styled(buf, p.style));
                }
                break 'outer;
            }
            buf.push_str(g);
            used += w;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, p.style));
        }
    }
    out.push(Span::styled("…", theme::faint()));
    out
}

/// 将展开后的命令明细按 `sai` 风格逐行做 Shell 语法高亮，正文行只读并在必要时附带操作条。
pub(super) fn push_highlighted_command_lines(
    entry_id: u64,
    cmd: &str,
    inner_w: usize,
    max_lines: usize,
    include_collapse: bool,
    out: &mut Vec<RenderRow>,
) {
    let lang = markdown::code::detect_shell_language(cmd);
    let body_w = inner_w.saturating_sub(2).max(4);
    let mut rendered_rows: Vec<Vec<Span<'static>>> = Vec::new();
    for (line_idx, raw_line) in cmd.lines().enumerate() {
        let expanded = linebreak::expand_tabs(raw_line, 4);
        let pieces = markdown::code::highlight_shell_line(&expanded, Some(lang));
        let folded = markdown::inline::fold(&pieces, body_w as u16);
        for (wrap_idx, fl) in folded.into_iter().enumerate() {
            let mut spans = vec![Span::styled("│ ", theme::faint())];
            if line_idx == 0 && wrap_idx == 0 {
                spans.push(Span::styled("$ ", theme::picked()));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.extend(fl.spans);
            rendered_rows.push(spans);
        }
    }
    let omitted = rendered_rows.len().saturating_sub(max_lines);
    for spans in rendered_rows.into_iter().take(max_lines) {
        out.push(RenderRow {
            entry_id,
            spans,
            links: Vec::new(),
            buttons: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 4,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if let Some(bar) = fold::action_bar(
        entry_id,
        ActionBarOptions {
            fold: (omitted > 0).then_some((FoldSection::ToolCommand, omitted)),
            show_full_file: false,
            show_collapse: include_collapse,
        },
    ) {
        out.push(bar);
    }
}

/// 渲染通用工具明细文本（路径/输出等），自动对 Windows/Linux 路径、URL 与状态关键词做分层着色。
pub(super) fn push_detail_lines(
    entry_id: u64,
    text: &str,
    style: Style,
    inner_w: usize,
    (section, max_lines): (FoldSection, usize),
    include_collapse: bool,
    out: &mut Vec<RenderRow>,
) {
    let mut rendered_rows: Vec<(Vec<Span<'static>>, Vec<(u16, u16, String)>)> = Vec::new();
    for raw in text.lines() {
        let expanded = linebreak::expand_tabs(raw, 4);
        let pieces = style_detail_line(&expanded, style);
        let folded = markdown::inline::fold(&pieces, inner_w.max(4) as u16);
        for fl in folded {
            let mut spans = Vec::with_capacity(1 + fl.spans.len());
            spans.push(Span::styled("│ ", theme::faint()));
            spans.extend(fl.spans);
            let links = fl
                .links
                .into_iter()
                .map(|(a, b, url)| (a.saturating_add(2), b.saturating_add(2), url))
                .collect();
            rendered_rows.push((spans, links));
        }
    }
    let omitted = rendered_rows.len().saturating_sub(max_lines);
    for (spans, links) in rendered_rows.into_iter().take(max_lines) {
        out.push(RenderRow {
            entry_id,
            spans,
            links,
            buttons: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if let Some(bar) = fold::action_bar(
        entry_id,
        ActionBarOptions {
            fold: (omitted > 0).then_some((section, omitted)),
            show_full_file: false,
            show_collapse: include_collapse,
        },
    ) {
        out.push(bar);
    }
}

/// 为单行工具输出做克制的语义着色：识别 Windows/Linux 文件路径、URL 以及常见错误/警告/通过状态词。
fn style_detail_line(line: &str, base: Style) -> Vec<markdown::inline::Piece> {
    use markdown::inline::Piece;
    use markdown::path_style::{self, SpanKind};

    let trimmed = line.trim_start();
    let line_base = if trimmed.starts_with("error")
        || trimmed.starts_with("FAILED")
        || trimmed.contains("... FAILED")
        || trimmed.starts_with("fatal:")
    {
        theme::error()
    } else if trimmed.starts_with("warning:") || trimmed.starts_with("WARN") {
        theme::warn()
    } else if trimmed.starts_with("test result: ok") || trimmed.ends_with("... ok") {
        base
    } else {
        base
    };

    let matches = path_style::find_urls_and_paths(line);
    if matches.is_empty() {
        return style_status_tail(line, line_base);
    }

    let mut pieces = Vec::new();
    let mut at = 0usize;
    for (start, end, kind) in matches {
        if start > at {
            pieces.extend(style_status_tail(&line[at..start], line_base));
        }
        let seg = &line[start..end];
        match kind {
            SpanKind::Url => pieces.push(Piece::linked(seg, theme::md_url(), seg)),
            SpanKind::Path => {
                let (target, _) = path_style::strip_line_suffix(seg);
                pieces.extend(path_style::style_path_pieces(seg, Some(target)));
            }
        }
        at = end;
    }
    if at < line.len() {
        pieces.extend(style_status_tail(&line[at..], line_base));
    }
    if pieces.is_empty() {
        pieces.push(Piece::new("", line_base));
    }
    pieces
}

fn style_status_tail(seg: &str, base: Style) -> Vec<markdown::inline::Piece> {
    use markdown::inline::Piece;
    if let Some(prefix) = seg.strip_suffix("... ok") {
        return vec![
            Piece::new(prefix, base),
            Piece::new("... ", theme::faint()),
            Piece::new("ok", theme::good()),
        ];
    }
    if let Some(prefix) = seg.strip_suffix("... FAILED") {
        return vec![
            Piece::new(prefix, base),
            Piece::new("... ", theme::faint()),
            Piece::new("FAILED", theme::error()),
        ];
    }
    vec![Piece::new(seg, base)]
}
