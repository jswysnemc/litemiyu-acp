//! 读取工具源码预览解析与 Shell 命令语法高亮截断辅助。

use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::diff;
use crate::linebreak;
use crate::markdown;
use crate::theme;

use super::{fold, RenderRow, RowAction, RowCopyMeta};
use crate::transcript::FoldSection;

pub(super) struct ReadPreviewLine {
    line_no: usize,
    text: String,
}

pub(super) struct ReadPreview {
    path: String,
    lines: Vec<ReadPreviewLine>,
    notices: Vec<String>,
}

pub(super) fn is_read_tool_call(name: &str, kind: &str) -> bool {
    kind == "read"
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "read_file" | "read" | "view_file" | "cat"
        )
}

pub(super) fn file_basename(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

pub(super) fn extract_read_path(title: &str, raw_input: Option<&Value>) -> Option<String> {
    if let Some(inp) = raw_input {
        for key in ["Path", "path", "AbsolutePath", "file_path", "filePath", "target_file"] {
            if let Some(p) = inp[key].as_str() {
                let decoded = crate::acp::decode_acp_cwd(p.trim());
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }
    let mut cleaned = title.trim();
    for prefix in ["Read ", "Reading ", "File ", "View "] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim();
            break;
        }
    }
    // 去掉标题尾部的行号区间描述（如 `foo.rs (1-50)`）
    let cleaned = cleaned
        .split_once(" (")
        .map_or(cleaned, |(head, _)| head.trim());
    let decoded = crate::acp::decode_acp_cwd(cleaned);
    if !decoded.is_empty()
        && (decoded.contains('\\') || decoded.contains('/') || decoded.contains('.'))
    {
        Some(decoded)
    } else {
        None
    }
}

fn extract_requested_read_range(raw_input: Option<&Value>) -> (Option<usize>, Option<usize>) {
    let Some(inp) = raw_input else {
        return (None, None);
    };
    let get_usize = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| inp[*k].as_u64())
            .map(|v| v as usize)
            .filter(|&v| v > 0)
    };
    let start = get_usize(&["StartLine", "start_line", "startLine", "offset"]);
    let end = get_usize(&["EndLine", "end_line", "endLine"]).or_else(|| {
        let limit = get_usize(&["limit", "Limit", "max_lines"])?;
        Some(start.unwrap_or(1).saturating_add(limit.saturating_sub(1)))
    });
    (start, end)
}

fn split_read_numbered_line(line: &str) -> Option<(usize, &str)> {
    let trimmed_start = line.trim_start_matches(' ');
    let (num_str, rest) = trimmed_start
        .split_once('\t')
        .or_else(|| trimmed_start.split_once(": "))
        .or_else(|| trimmed_start.split_once(" | "))?;
    let num_str = num_str.trim();
    if num_str.is_empty() || !num_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let no = num_str.parse::<usize>().ok()?;
    Some((no, rest))
}

fn parse_read_output_only(output: &str, path: &str, raw_input: Option<&Value>) -> Option<ReadPreview> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. 兼容 `sai` text-page JSON 格式
    if trimmed.starts_with('{')
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
            && let Some(content) = val["content"].as_str() {
                let p = val["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(crate::acp::decode_acp_cwd)
                    .unwrap_or_else(|| path.to_string());
                let offset = val["offset"].as_u64().unwrap_or(1).max(1) as usize;
                let mut lines = Vec::new();
                for (i, raw_line) in content.lines().enumerate() {
                    if let Some((no, rest)) = split_read_numbered_line(raw_line) {
                        lines.push(ReadPreviewLine {
                            line_no: no,
                            text: rest.to_string(),
                        });
                    } else {
                        lines.push(ReadPreviewLine {
                            line_no: offset + i,
                            text: raw_line.to_string(),
                        });
                    }
                }
                let mut notices = Vec::new();
                if val["truncated"].as_bool() == Some(true)
                    && let Some(next) = val["next"].as_u64() {
                        notices.push(format!("后续内容起始行：{next}"));
                    }
                if !lines.is_empty() {
                    return Some(ReadPreview {
                        path: p,
                        lines,
                        notices,
                    });
                }
            }

    // 2. 识别带行号文本（`N\tcode` / `N: code` / `N | code`，允许首行 `Showing lines ...` 或尾部提醒）
    let mut numbered = Vec::new();
    let mut notices = Vec::new();
    let mut saw_trailing_notice = false;
    let mut valid_numbered = true;
    for (idx, line) in trimmed.lines().enumerate() {
        if let Some((no, rest)) = split_read_numbered_line(line) {
            if saw_trailing_notice {
                valid_numbered = false;
                break;
            }
            numbered.push(ReadPreviewLine {
                line_no: no,
                text: rest.to_string(),
            });
        } else if idx == 0 && (line.starts_with("Showing lines ") || line.starts_with("File: ")) {
            notices.push(line.to_string());
        } else if !numbered.is_empty() {
            saw_trailing_notice = true;
            if !line.trim().is_empty() {
                notices.push(line.to_string());
            }
        } else {
            valid_numbered = false;
            break;
        }
    }
    if valid_numbered && !numbered.is_empty() {
        return Some(ReadPreview {
            path: path.to_string(),
            lines: numbered,
            notices,
        });
    }

    // 3. 纯源码文本输出（如未带行号前缀）：按起始行号补齐行号并着色
    let (req_start, _) = extract_requested_read_range(raw_input);
    let base = req_start.unwrap_or(1);
    let lines: Vec<ReadPreviewLine> = output
        .lines()
        .enumerate()
        .map(|(i, l)| ReadPreviewLine {
            line_no: base + i,
            text: l.to_string(),
        })
        .collect();
    (!lines.is_empty()).then(|| ReadPreview {
        path: path.to_string(),
        lines,
        notices: Vec::new(),
    })
}

pub(super) fn read_header_range_badge(output: &str, raw_input: Option<&Value>) -> Option<String> {
    if let Some(preview) = parse_read_output_only(output, "", raw_input)
        && let (Some(first), Some(last)) = (preview.lines.first(), preview.lines.last()) {
            return Some(if first.line_no == last.line_no {
                format!("L{}", first.line_no)
            } else {
                format!("{}–{}", first.line_no, last.line_no)
            });
        }
    match extract_requested_read_range(raw_input) {
        (Some(s), Some(e)) if s == e => Some(format!("L{s}")),
        (Some(s), Some(e)) => Some(format!("{s}–{e}")),
        (Some(s), None) => Some(format!("L{s}+")),
        _ => None,
    }
}

/// 展开时构建读取预览：优先解析工具返回文本，若 ACP 后端未回传正文则按路径/区间安全回读本地文件兜底。
pub(super) fn build_read_preview(output: &str, path: &str, raw_input: Option<&Value>) -> Option<ReadPreview> {
    if let Some(parsed) = parse_read_output_only(output, path, raw_input) {
        return Some(parsed);
    }
    if path.is_empty() {
        return None;
    }
    let fs_path = std::path::Path::new(path);
    let meta = std::fs::metadata(fs_path).ok()?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
        return None;
    }
    let content = std::fs::read_to_string(fs_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();
    if all_lines.is_empty() {
        return Some(ReadPreview {
            path: path.to_string(),
            lines: Vec::new(),
            notices: vec!["(空文件)".to_string()],
        });
    }
    let (req_start, req_end) = extract_requested_read_range(raw_input);
    let start = req_start.unwrap_or(1).clamp(1, all_lines.len());
    let default_end = start.saturating_add(119);
    let end = req_end
        .unwrap_or(default_end)
        .min(start.saturating_add(159))
        .min(all_lines.len());
    let lines = (start..=end)
        .map(|no| ReadPreviewLine {
            line_no: no,
            text: all_lines[no - 1].to_string(),
        })
        .collect();
    let mut notices = Vec::new();
    if end < all_lines.len() {
        notices.push(format!("共 {} 行（当前预览 {start}–{end} 行）", all_lines.len()));
    }
    Some(ReadPreview {
        path: path.to_string(),
        lines,
        notices,
    })
}

/// 参考 `sai/read_file.rs` 与 `diff.rs` 渲染带中性灰行号 + 文件语言语法高亮的读取预览。
pub(super) fn push_read_preview_rows(
    entry_id: u64,
    preview: &ReadPreview,
    width: u16,
    max_visual_lines: usize,
    out: &mut Vec<RenderRow>,
) {
    let max_no = preview
        .lines
        .iter()
        .map(|l| l.line_no)
        .max()
        .unwrap_or(1);
    let num_w = max_no.to_string().len().max(3);
    // 左内收 1 列 + 行号 num_w + 2 空格间隔
    let prefix_w = 1 + num_w + 2;
    let code_w = usize::from(width).saturating_sub(prefix_w + 1).max(8);

    let lang = diff::language_from_path(std::path::Path::new(&preview.path))
        .and_then(markdown::code::find_language);
    let mut hl = markdown::code::Highlighter::new(lang);

    let mut emitted = 0usize;
    let mut omitted = 0usize;
    for line in &preview.lines {
        let expanded = linebreak::expand_tabs(&line.text, 4);
        let pieces = hl.line(&expanded);
        let pieces = if pieces.is_empty() {
            vec![markdown::inline::Piece::new("", theme::text())]
        } else {
            pieces
        };
        for (wrap_idx, fl) in markdown::inline::fold(&pieces, code_w as u16)
            .into_iter()
            .enumerate()
        {
            emitted += 1;
            if emitted > max_visual_lines {
                omitted += 1;
                continue;
            }
            let gutter = if wrap_idx == 0 {
                format!(" {:>num_w$}  ", line.line_no)
            } else {
                " ".repeat(prefix_w)
            };
            let mut spans = Vec::with_capacity(1 + fl.spans.len());
            spans.push(Span::styled(gutter, theme::faint()));
            spans.extend(fl.spans);
            out.push(RenderRow {
                entry_id,
                spans,
                links: Vec::new(),
                action: RowAction::Toggle,
                copy_meta: RowCopyMeta {
                    content_left: u16::try_from(prefix_w).unwrap_or(u16::MAX),
                    decorative: false,
                    join_previous: wrap_idx > 0,
                },
            });
        }
    }
    if omitted > 0 {
        out.push(fold::marker(entry_id, FoldSection::ToolReadPreview, omitted));
    }
    for notice in &preview.notices {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::raw(" "),
                Span::styled(linebreak::clip(notice, width.saturating_sub(2)), theme::faint()),
            ],
            links: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 1,
                decorative: true,
                join_previous: false,
            },
        });
    }
}

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

/// 将展开后的命令明细按 `sai` 风格逐行做 Shell 语法高亮，续行对齐到命令正文列。
pub(super) fn push_highlighted_command_lines(
    entry_id: u64,
    cmd: &str,
    inner_w: usize,
    max_lines: usize,
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
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 4,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if omitted > 0 {
        out.push(fold::marker(entry_id, FoldSection::ToolCommand, omitted));
    }
}

pub(super) fn push_detail_lines(
    entry_id: u64,
    text: &str,
    style: Style,
    inner_w: usize,
    (section, max_lines): (FoldSection, usize),
    out: &mut Vec<RenderRow>,
) {
    let mut wrapped = Vec::new();
    for raw in text.lines() {
        let expanded = linebreak::expand_tabs(raw, 4);
        let cells: Vec<&str> = expanded.graphemes(true).collect();
        for r in linebreak::lines(&cells, inner_w) {
            wrapped.push(cells[r].concat());
        }
    }
    let omitted = wrapped.len().saturating_sub(max_lines);
    for line in wrapped.into_iter().take(max_lines) {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::styled("│ ", theme::faint()),
                Span::styled(line, style),
            ],
            links: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 2,
                decorative: false,
                join_previous: false,
            },
        });
    }
    if omitted > 0 {
        out.push(fold::marker(entry_id, section, omitted));
    }
}
