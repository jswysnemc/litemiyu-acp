//! 读取工具源码预览解析、本地完整文件缓存加载与带行号语法高亮渲染。

use ratatui::text::Span;
use serde_json::Value;

use crate::diff;
use crate::linebreak;
use crate::markdown;
use crate::theme;
use crate::transcript::FoldSection;

use super::fold::{self, ActionBarOptions};
use super::{MdCache, RenderRow, RowAction, RowCopyMeta};

pub(super) struct ReadPreviewLine {
    pub(super) line_no: usize,
    pub(super) text: String,
}

pub(super) struct ReadPreview {
    pub(super) path: String,
    pub(super) lines: Vec<ReadPreviewLine>,
    pub(super) notices: Vec<String>,
    /// 当前仅展示了文件的局部区间且本地完整文件存在更多内容（用于决定是否显示「查看完整文件」）
    pub(super) can_view_full_file: bool,
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
        for key in [
            "Path",
            "path",
            "AbsolutePath",
            "file_path",
            "filePath",
            "target_file",
        ] {
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
        && let Some(content) = val["content"].as_str()
    {
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
            && let Some(next) = val["next"].as_u64()
        {
            notices.push(format!("后续内容起始行：{next}"));
        }
        if !lines.is_empty() {
            return Some(ReadPreview {
                path: p,
                lines,
                notices,
                can_view_full_file: false,
            });
        }
    }

    // 2. 识别带行号文本（`N\tcode` / `N: code` / `N | code`）
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
            can_view_full_file: false,
        });
    }

    // 3. 纯源码文本输出：按起始行号补齐行号并着色
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
        can_view_full_file: false,
    })
}

pub(super) fn read_header_range_badge(output: &str, raw_input: Option<&Value>) -> Option<String> {
    if let Some(preview) = parse_read_output_only(output, "", raw_input)
        && let (Some(first), Some(last)) = (preview.lines.first(), preview.lines.last())
    {
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

/// 【读取工具】【构建预览】取消 120/160 行源头截断，完整加载目标区间或本地全文并交给折叠系统按需展开。
///
/// 参数:
/// - `output`: ACP 工具返回的文本输出
/// - `path`: 解码后的本地文件路径
/// - `raw_input`: 工具调用入参（用于提取起始/结束行号）
/// - `full_file`: 是否已由用户切换为「查看完整文件」模式
/// - `cache`: 渲染缓存（缓存本地文件内容，避免每帧重复读盘）
///
/// 返回:
/// - 构建完成的 `ReadPreview`（包含是否可进一步「查看完整文件」标记）
pub(super) fn build_read_preview(
    output: &str,
    path: &str,
    raw_input: Option<&Value>,
    full_file: bool,
    cache: &mut MdCache,
) -> Option<ReadPreview> {
    let local_lines = if !path.is_empty() {
        cache.read_local_file(path)
    } else {
        None
    };

    // 1. 用户主动点击「查看完整文件」时，直接加载本地文件全部行
    if full_file
        && let Some(all_lines) = local_lines.as_ref()
    {
        if all_lines.is_empty() {
            return Some(ReadPreview {
                path: path.to_string(),
                lines: Vec::new(),
                notices: vec!["(空文件)".to_string()],
                can_view_full_file: false,
            });
        }
        let lines = all_lines
            .iter()
            .enumerate()
            .map(|(i, text)| ReadPreviewLine {
                line_no: i + 1,
                text: text.clone(),
            })
            .collect();
        return Some(ReadPreview {
            path: path.to_string(),
            lines,
            notices: Vec::new(),
            can_view_full_file: false,
        });
    }

    // 2. 优先使用工具返回的完整输出，并与本地文件总行数比对判断是否为局部读取
    if let Some(mut parsed) = parse_read_output_only(output, path, raw_input) {
        if let Some(all_lines) = local_lines.as_ref()
            && !all_lines.is_empty()
            && let (Some(first), Some(last)) = (parsed.lines.first(), parsed.lines.last())
        {
            parsed.can_view_full_file = first.line_no > 1 || last.line_no < all_lines.len();
        }
        return Some(parsed);
    }

    // 3. 工具未回传正文时按缓存本地文件回读目标区间（不硬切 120/160 行，交由折叠系统按 24 行起步展开）
    let all_lines = local_lines?;
    if all_lines.is_empty() {
        return Some(ReadPreview {
            path: path.to_string(),
            lines: Vec::new(),
            notices: vec!["(空文件)".to_string()],
            can_view_full_file: false,
        });
    }
    let (req_start, req_end) = extract_requested_read_range(raw_input);
    let start = req_start.unwrap_or(1).clamp(1, all_lines.len());
    let end = req_end
        .map(|e| e.clamp(start, all_lines.len()))
        .unwrap_or(all_lines.len());
    let lines = (start..=end)
        .map(|no| ReadPreviewLine {
            line_no: no,
            text: all_lines[no - 1].clone(),
        })
        .collect();
    let can_view_full_file = start > 1 || end < all_lines.len();
    Some(ReadPreview {
        path: path.to_string(),
        lines,
        notices: Vec::new(),
        can_view_full_file,
    })
}

/// 渲染带中性灰行号与语法高亮的读取预览行，并在尾部拼接统一的智能展开/查看完整文件/收起操作条。
///
/// 参数:
/// - `entry_id`: 时间线条目 ID
/// - `preview`: 解析完成的源码预览
/// - `width`: 正文可用列宽
/// - `max_visual_lines`: 当前允许展示的最大视觉行数
/// - `out`: 输出渲染行集合
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
                buttons: Vec::new(),
                action: RowAction::Toggle,
                copy_meta: RowCopyMeta {
                    content_left: u16::try_from(prefix_w).unwrap_or(u16::MAX),
                    decorative: false,
                    join_previous: wrap_idx > 0,
                },
            });
        }
    }
    for notice in &preview.notices {
        out.push(RenderRow {
            entry_id,
            spans: vec![
                Span::raw(" "),
                Span::styled(linebreak::clip(notice, width.saturating_sub(2)), theme::faint()),
            ],
            links: Vec::new(),
            buttons: Vec::new(),
            action: RowAction::Toggle,
            copy_meta: RowCopyMeta {
                content_left: 1,
                decorative: true,
                join_previous: false,
            },
        });
    }
    if let Some(bar) = fold::action_bar(
        entry_id,
        ActionBarOptions {
            fold: (omitted > 0).then_some((FoldSection::ToolReadPreview, omitted)),
            show_full_file: preview.can_view_full_file,
            show_collapse: true,
        },
    ) {
        out.push(bar);
    }
}
