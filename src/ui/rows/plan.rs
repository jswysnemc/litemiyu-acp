//! 正文时间线中的可折叠任务进度卡片（支持进度条、状态着色与中文折行）。

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::acp::PlanItem;
use crate::linebreak;
use crate::theme;
use crate::todo::{self, TodoStatus};

use super::{fold, RenderRow, RowAction, RowCopyMeta};

/// 【正文】【绘制任务卡片】将任务清单渲染为默认单行、可点击展开/折叠的紧凑进度摘要。
///
/// 参数:
/// - `entry_id`: 时间线条目 ID
/// - `items`: 任务步骤列表
/// - `expanded`: 当前卡片是否展开
/// - `width`: 正文可用列宽
/// - `busy`: 当前会话是否处于生成活跃态
/// - `tick`: 动效节拍（驱动进行中旋转符号）
/// - `out`: 输出渲染行集合
pub(super) fn build_plan(
    entry_id: u64,
    items: &[PlanItem],
    expanded: bool,
    width: u16,
    busy: bool,
    tick: usize,
    out: &mut Vec<RenderRow>,
) {
    if items.is_empty() {
        return;
    }
    let counts = todo::count_items(items);
    let all_done = counts.all_done();
    let animate_in_progress = busy && counts.in_progress > 0;
    let bar_w = if width >= 54 { 8 } else if width >= 38 { 5 } else { 0 };
    let (filled, empty) = todo::progress_bar(&counts, bar_w);

    // 1. 构造卡片标题行（支持点击折叠/展开，并展示完成计数与迷你进度条）
    let lead_icon = if all_done { "✓ " } else { "◈ " };
    let lead_style = if all_done {
        theme::good()
    } else if counts.failed > 0 {
        theme::error()
    } else {
        theme::side_title()
    };
    let count_text = format!(" · {}/{}", counts.finished(), counts.total);
    let fold_mark = if expanded { " ▾" } else { " ▸" };

    let mut header_spans = vec![
        Span::styled(lead_icon, lead_style),
        Span::styled("任务清单", lead_style),
        Span::styled(count_text.clone(), if all_done { theme::good() } else { theme::dim() }),
    ];
    let mut header_used = lead_icon.width() + "任务清单".width() + count_text.width() + fold_mark.width();

    if bar_w > 0 {
        header_spans.push(Span::raw("  "));
        header_spans.push(Span::styled(
            filled,
            if all_done { theme::good() } else { theme::accent() },
        ));
        header_spans.push(Span::styled(empty, theme::faint()));
        header_used += 2 + bar_w;
    }

    // 2. 折叠态且空间充足时，在标题尾部追加当前聚焦步骤摘要
    if !expanded
        && let Some(active) = todo::focus_item(items)
    {
        let room = usize::from(width).saturating_sub(header_used + 5);
        if room >= 8 {
            let st = todo::parse_status(&active.status);
            let (sym, sym_style, _) =
                todo::status_visual(st, animate_in_progress && st == TodoStatus::InProgress, tick);
            header_spans.push(Span::styled(" · ", theme::faint()));
            header_spans.push(Span::styled(format!("{sym} "), sym_style));
            header_spans.push(Span::styled(
                linebreak::clip(&active.content, room as u16),
                theme::dim(),
            ));
        }
    }
    header_spans.push(Span::styled(fold_mark, theme::faint()));

    out.push(RenderRow {
        entry_id,
        spans: header_spans,
        links: Vec::new(),
        buttons: Vec::new(),
        action: RowAction::Toggle,
        copy_meta: RowCopyMeta::default(),
    });

    if !expanded {
        return;
    }

    // 3. 展开态逐条渲染步骤，正文行只读防误收起，底部附带统一收起操作条
    let inner_w = usize::from(width.saturating_sub(4)).max(8);
    for item in items {
        let status = todo::parse_status(&item.status);
        let animate = busy && status == TodoStatus::InProgress;
        let (sym, sym_style, text_style) = todo::status_visual(status, animate, tick);
        let cells: Vec<&str> = item.content.graphemes(true).collect();
        let wrapped = linebreak::lines(&cells, inner_w);
        for (wrap_idx, range) in wrapped.into_iter().enumerate() {
            let seg = cells[range].concat();
            let spans = if wrap_idx == 0 {
                vec![
                    Span::styled("  ", Style::new()),
                    Span::styled(format!("{sym} "), sym_style),
                    Span::styled(seg, text_style),
                ]
            } else {
                vec![
                    Span::styled("    ", Style::new()),
                    Span::styled(seg, text_style),
                ]
            };
            out.push(RenderRow {
                entry_id,
                spans,
                links: Vec::new(),
                buttons: Vec::new(),
                action: RowAction::Toggle,
                copy_meta: RowCopyMeta {
                    content_left: 4,
                    decorative: false,
                    join_previous: wrap_idx > 0,
                },
            });
        }
    }
    out.push(fold::collapse_bar(entry_id));
}
