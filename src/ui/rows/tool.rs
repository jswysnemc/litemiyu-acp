//! 工具调用卡片、Diff 展开与命令/读取工具详情行构建。

use ratatui::text::Span;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::acp::ToolDiff;
use crate::diff;
use crate::linebreak;
use crate::theme;
use crate::transcript::{FoldSection, Transcript};

use super::tool_meta::{
    build_read_preview, clip_highlighted_shell, extract_command_text, extract_read_path,
    file_basename, is_read_tool_call, push_detail_lines, push_highlighted_command_lines,
    push_read_preview_rows, read_header_range_badge,
};
use super::{fold, MdCache, RenderRow, RowAction, RowCopyMeta, SPINNER};

#[allow(clippy::too_many_arguments)]
pub(super) fn build_tool(
    entry_id: u64,
    transcript: &Transcript,
    name: &str,
    title: &str,
    kind: &str,
    status: &str,
    raw_input: Option<&Value>,
    output: &str,
    diffs: &[ToolDiff],
    expanded: bool,
    width: u16,
    tick: usize,
    cache: &mut MdCache,
    out: &mut Vec<RenderRow>,
) {
    let (icon, icon_style) = match status {
        "completed" => ("✓", theme::good()),
        "failed" | "error" | "cancelled" => ("✕", theme::error()),
        "pending" => ("◈", theme::warn()),
        _ => (SPINNER[tick % SPINNER.len()], theme::accent()),
    };
    let kind_label = match kind {
        "read" => "读取",
        "edit" => "编辑",
        "execute" => "执行",
        "search" => "搜索",
        "fetch" => "网络",
        _ if name.eq_ignore_ascii_case("ask_question") => "提问",
        _ => name,
    };
    let clean_title = title.replace('\n', " ");
    let is_read_tool = is_read_tool_call(name, kind);
    let read_path = if is_read_tool {
        extract_read_path(&clean_title, raw_input)
    } else {
        None
    };
    let read_range_badge = if is_read_tool {
        read_header_range_badge(output, raw_input)
    } else {
        None
    };

    let summary = if is_read_tool {
        let display_name = read_path
            .as_deref()
            .map(file_basename)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if clean_title.eq_ignore_ascii_case(name) || clean_title.is_empty() {
                    name.to_string()
                } else {
                    clean_title.clone()
                }
            });
        format!("{kind_label} · {display_name}")
    } else if clean_title.eq_ignore_ascii_case(name) || clean_title.is_empty() {
        format!("{kind_label} · {name}")
    } else {
        format!("{kind_label} · {clean_title}")
    };

    // 预计算并缓存每个 diff 的统计，避免每帧对历史编辑记录重复执行 Myers Diff
    let computed_diffs: Vec<(&ToolDiff, diff::ComputedDiff)> = diffs
        .iter()
        .enumerate()
        .map(|(idx, d)| (d, cache.compute_diff(entry_id, idx, d)))
        .collect();
    let total_added: usize = computed_diffs.iter().map(|(_, c)| c.added).sum();
    let total_removed: usize = computed_diffs.iter().map(|(_, c)| c.removed).sum();

    let has_detail = !computed_diffs.is_empty()
        || read_path.is_some()
        || raw_input.is_some()
        || !output.trim().is_empty();
    let fold_mark = if !has_detail {
        ""
    } else if expanded {
        " ▾"
    } else {
        " ▸"
    };
    let text_style = if status == "failed" || status == "error" {
        theme::error()
    } else {
        theme::dim()
    };

    let mut stat_spans = Vec::new();
    let mut stat_w = 0usize;
    if (total_added > 0 || total_removed > 0) && status != "failed" && status != "error" {
        let add_s = format!("+{total_added}");
        let rem_s = format!("-{total_removed}");
        stat_w = 3 + add_s.len() + 1 + rem_s.len();
        stat_spans.push(Span::styled(" · ", theme::faint()));
        stat_spans.push(Span::styled(add_s, theme::added()));
        stat_spans.push(Span::raw(" "));
        stat_spans.push(Span::styled(rem_s, theme::removed()));
    } else if let Some(range) = &read_range_badge
        && status != "failed" && status != "error" {
            stat_w = 3 + range.width();
            stat_spans.push(Span::styled(" · ", theme::faint()));
            stat_spans.push(Span::styled(range.clone(), theme::faint()));
        }

    let summary_room = usize::from(width)
        .saturating_sub(5 + stat_w)
        .max(8) as u16;
    let exec_command = extract_command_text(name, kind, title, raw_input);
    let mut header_spans = if let Some(cmd) = exec_command.as_deref() {
        let one_line_cmd = cmd.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ; ");
        let verb_style = if status == "failed" || status == "error" {
            theme::error()
        } else {
            theme::model()
        };
        let mut spans = vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(kind_label.to_string(), verb_style),
            Span::styled(" · ", theme::faint()),
            Span::styled("$ ", theme::picked()),
        ];
        let used_prefix = 2 + kind_label.width() + 3 + 2;
        let cmd_room = usize::from(width)
            .saturating_sub(used_prefix + stat_w + fold_mark.width())
            .max(8);
        spans.extend(clip_highlighted_shell(&one_line_cmd, cmd_room));
        spans
    } else {
        vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(linebreak::clip(&summary, summary_room), text_style),
        ]
    };
    header_spans.extend(stat_spans);
    header_spans.push(Span::styled(fold_mark, theme::faint()));

    out.push(RenderRow {
        entry_id,
        spans: header_spans,
        links: Vec::new(),
        action: if has_detail { RowAction::Toggle } else { RowAction::None },
        copy_meta: RowCopyMeta::default(),
    });

    if !expanded {
        return;
    }

    let inner_w = usize::from(width.saturating_sub(2)).max(1);
    let limit = |section, base| transcript.fold_reveal(entry_id, section).effective_limit(base);
    let output_limit = |base| (FoldSection::ToolOutput, limit(FoldSection::ToolOutput, base));

    // 1. 若有结构化 Diff，展开后展示 `sai` 风格语法高亮 + 行内差异强调 Diff
    if !computed_diffs.is_empty() {
        let multi_file = computed_diffs.len() > 1;
        for (idx, (td, computed)) in computed_diffs.iter().enumerate() {
            if idx > 0 {
                out.push(RenderRow {
                    entry_id,
                    spans: Vec::new(),
                    links: Vec::new(),
                    action: RowAction::Toggle,
                    copy_meta: RowCopyMeta::default(),
                });
            }
            if !td.path.is_empty() {
                let path_shown = linebreak::clip(&td.path, inner_w.saturating_sub(14) as u16);
                let path_w = u16::try_from(path_shown.width()).unwrap_or(0);
                let mut sub = vec![
                    Span::styled(" • ", theme::faint()),
                    Span::styled(path_shown, theme::md_url()),
                ];
                if multi_file {
                    sub.push(Span::styled(" (", theme::faint()));
                    sub.push(Span::styled(format!("+{}", computed.added), theme::added()));
                    sub.push(Span::raw(" "));
                    sub.push(Span::styled(format!("-{}", computed.removed), theme::removed()));
                    sub.push(Span::styled(")", theme::faint()));
                }
                out.push(RenderRow {
                    entry_id,
                    spans: sub,
                    links: vec![(3, 3 + path_w, td.path.clone())],
                    action: RowAction::Toggle,
                    copy_meta: RowCopyMeta::default(),
                });
            }
            if computed.rows.is_empty() {
                out.push(RenderRow {
                    entry_id,
                    spans: vec![
                        Span::raw(" "),
                        Span::styled("(无文本变更)", theme::faint()),
                    ],
                    links: Vec::new(),
                    action: RowAction::Toggle,
                    copy_meta: RowCopyMeta::default(),
                });
            } else {
                let section = FoldSection::ToolDiff(idx);
                let cap = limit(section, diff::DIFF_EXPANDED_MAX_VISUAL_LINES);
                let (diff_lines, omitted) = diff::render_lines_with_cap(computed, &td.path, width, true, cap);
                for spans in diff_lines {
                    out.push(RenderRow {
                        entry_id,
                        spans,
                        links: Vec::new(),
                        action: RowAction::Toggle,
                        copy_meta: RowCopyMeta::default(),
                    });
                }
                if omitted > 0 {
                    out.push(fold::marker(entry_id, section, omitted));
                }
            }
        }
        let trimmed_out = output.trim();
        if !trimmed_out.is_empty() {
            push_detail_lines(entry_id, trimmed_out, theme::dim(), inner_w, output_limit(12), out);
        }
        return;
    }

    // 2. 文件读取工具：与 Diff 一致的点击展开逻辑 + 参考 `sai/read_file.rs` 的带行号语法高亮代码预览
    if is_read_tool {
        let path_str = read_path.as_deref().unwrap_or("");
        if !path_str.is_empty() {
            let path_shown = linebreak::clip(path_str, inner_w.saturating_sub(4) as u16);
            let path_w = u16::try_from(path_shown.width()).unwrap_or(0);
            out.push(RenderRow {
                entry_id,
                spans: vec![
                    Span::styled(" • ", theme::faint()),
                    Span::styled(path_shown, theme::md_url()),
                ],
                links: vec![(3, 3 + path_w, path_str.to_string())],
                action: RowAction::Toggle,
                copy_meta: RowCopyMeta::default(),
            });
        }
        if let Some(preview) = build_read_preview(output, path_str, raw_input) {
            push_read_preview_rows(entry_id, &preview, width, limit(FoldSection::ToolReadPreview, 160), out);
        } else if !output.trim().is_empty() {
            push_detail_lines(entry_id, output.trim(), theme::dim(), inner_w, output_limit(40), out);
        } else {
            out.push(RenderRow {
                entry_id,
                spans: vec![
                    Span::raw(" "),
                    Span::styled("(无预览内容)", theme::faint()),
                ],
                links: Vec::new(),
                action: RowAction::Toggle,
                copy_meta: RowCopyMeta::default(),
            });
        }
        return;
    }

    // 3. 普通工具（命令执行、搜索等）
    if let Some(cmd) = exec_command {
        push_highlighted_command_lines(entry_id, &cmd, inner_w, limit(FoldSection::ToolCommand, 8), out);
    } else if let Some(input) = raw_input
        && let Some(path) = input["Path"]
            .as_str()
            .or_else(|| input["path"].as_str())
        {
            let section = FoldSection::ToolPath;
            push_detail_lines(entry_id, path, theme::md_url(), inner_w, (section, limit(section, 2)), out);
        }
    let trimmed_out = output.trim();
    if !trimmed_out.is_empty() {
        push_detail_lines(entry_id, trimmed_out, theme::dim(), inner_w, output_limit(24), out);
    }
}
