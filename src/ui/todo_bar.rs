//! 底部固定任务清单停靠条（宽屏与窄屏统一常驻底部，支持单行与展开清单切换）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::acp::PlanItem;
use crate::linebreak;
use crate::theme;
use crate::todo::{self, TodoStatus};

use super::HoverTarget;

/// 展开态停靠清单最多展示的步骤行数。
const MAX_EXPANDED_ITEMS: usize = 6;

/// 展开态精确窗口布局结果（保证步骤行与上下省略提示严格落在给定高度内）。
struct ExpandedWindow<'a> {
    items: &'a [PlanItem],
    above: usize,
    below: usize,
    show_above: bool,
    show_below: bool,
}

impl ExpandedWindow<'_> {
    /// 返回展开内容区占用的总行数（不含标题行）。
    fn rows(&self) -> usize {
        self.items.len() + usize::from(self.show_above) + usize::from(self.show_below)
    }
}

/// 在给定可用内容行数内计算展开态步骤切片与省略提示，避免矮终端静默丢项或覆盖当前步骤。
///
/// 参数:
/// - `items`: 完整任务列表
/// - `body_cap`: 除去标题行后最多可分配的内容行数
///
/// 返回:
/// - 严格满足 `rows() <= body_cap` 的展开布局结果
fn compute_expanded_window(items: &[PlanItem], body_cap: usize) -> ExpandedWindow<'_> {
    if items.is_empty() || body_cap == 0 {
        return ExpandedWindow {
            items: &[],
            above: 0,
            below: 0,
            show_above: false,
            show_below: false,
        };
    }
    let max_items = body_cap.min(MAX_EXPANDED_ITEMS);
    if items.len() <= max_items {
        return ExpandedWindow {
            items,
            above: 0,
            below: 0,
            show_above: false,
            show_below: false,
        };
    }
    // 1. 优先尝试完整保留当前视口步骤以及真实的上下省略提示行
    for limit in (1..=max_items).rev() {
        let (window, above, below) = todo::visible_window(items, limit);
        let needed = window.len() + usize::from(above > 0) + usize::from(below > 0);
        if needed <= body_cap {
            return ExpandedWindow {
                items: window,
                above,
                below,
                show_above: above > 0,
                show_below: below > 0,
            };
        }
    }
    // 2. 极矮高度（body_cap <= 2）时至少保底展示当前聚焦的 1 条步骤，余出 1 行时再显示省略提示
    let (window, above, below) = todo::visible_window(items, 1);
    let show_below = body_cap >= 2 && below > 0;
    let show_above = body_cap >= 2 && !show_below && above > 0;
    ExpandedWindow {
        items: window,
        above,
        below,
        show_above,
        show_below,
    }
}

/// 判断当前布局是否需要渲染底部固定任务条，并返回所需行数。
///
/// 参数:
/// - `items`: 当前会话最新任务清单
/// - `expanded`: 是否处于展开态（false 时固定保留单行摘要）
/// - `main_height`: 主对话栏总高度
///
/// 返回:
/// - 需占用的高度行数（`0` 表示不渲染）
pub(super) fn measure_height(items: &[PlanItem], expanded: bool, main_height: u16) -> u16 {
    if items.is_empty() || main_height < 14 {
        return 0;
    }
    if !expanded {
        return 1;
    }
    let max_h = usize::from(main_height.saturating_sub(10)).clamp(2, MAX_EXPANDED_ITEMS + 3);
    let win = compute_expanded_window(items, max_h.saturating_sub(1));
    (1 + win.rows()).min(max_h) as u16
}

/// 【底部任务栏】【绘制停靠视图】在状态行上方渲染单行摘要或展开态紧凑清单，返回点击切换热区。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `area`: 停靠任务栏矩形区域
/// - `items`: 当前会话最新任务清单
/// - `expanded`: 是否处于展开态
/// - `busy`: 当前会话是否处于生成活跃态
/// - `tick`: 动效节拍
/// - `hover`: 当前鼠标悬停目标
///
/// 返回:
/// - 用于点击切换展开/收起的热区 `Rect`
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    items: &[PlanItem],
    expanded: bool,
    busy: bool,
    tick: usize,
    hover: &HoverTarget,
) -> Rect {
    if area.width < 12 || area.height == 0 || items.is_empty() {
        return Rect::default();
    }

    let counts = todo::count_items(items);
    let all_done = counts.all_done();
    let is_hover = matches!(hover, HoverTarget::TodoBarToggle);
    let row_bg = if is_hover {
        Style::new().bg(theme::palette().row_focus_bg)
    } else {
        Style::new()
    };

    // 1. 构造首行标题栏（折叠态带当前进行中步骤摘要，展开态带进度条与收起提示）
    let right_hint = if expanded {
        "▾ Ctrl+T 收起"
    } else {
        "▸ Ctrl+T 展开"
    };
    let right_w = right_hint.width();
    let lead_icon = if all_done { "✓ " } else { "◈ " };
    let lead_style = if all_done {
        theme::good().patch(row_bg)
    } else if counts.failed > 0 {
        theme::error().patch(row_bg)
    } else {
        theme::accent_bold().patch(row_bg)
    };
    let badge = format!("任务 {}/{}", counts.finished(), counts.total);

    let mut header_spans = vec![
        Span::styled(lead_icon, lead_style),
        Span::styled(badge.clone(), lead_style),
    ];
    let mut left_used = lead_icon.width() + badge.width();
    let total_w = usize::from(area.width);
    let show_right = total_w > left_used + right_w + 4;
    let left_budget = if show_right {
        total_w.saturating_sub(right_w + 2)
    } else {
        total_w
    };

    if expanded {
        let bar_w = if left_budget >= left_used + 10 { 8 } else { 0 };
        if bar_w > 0 {
            let (filled, empty) = todo::progress_bar(&counts, bar_w);
            header_spans.push(Span::styled("  ", row_bg));
            header_spans.push(Span::styled(
                filled,
                if all_done {
                    theme::good().patch(row_bg)
                } else {
                    theme::accent().patch(row_bg)
                },
            ));
            header_spans.push(Span::styled(empty, theme::faint().patch(row_bg)));
            left_used += 2 + bar_w;
        }
    } else if let Some(active) = todo::focus_item(items) {
        let text_room = left_budget.saturating_sub(left_used + 5);
        if text_room >= 6 {
            let st = todo::parse_status(&active.status);
            let (sym, sym_style, text_style) =
                todo::status_visual(st, busy && st == TodoStatus::InProgress, tick);
            let clipped = linebreak::clip(&active.content, text_room as u16);
            left_used += 3 + sym.width() + 1 + clipped.width();
            header_spans.push(Span::styled(" · ", theme::faint().patch(row_bg)));
            header_spans.push(Span::styled(format!("{sym} "), sym_style.patch(row_bg)));
            header_spans.push(Span::styled(clipped, text_style.patch(row_bg)));
        }
    }

    if show_right {
        let gap = total_w.saturating_sub(left_used + right_w);
        if gap > 0 {
            header_spans.push(Span::styled(" ".repeat(gap), row_bg));
        }
        header_spans.push(Span::styled(
            right_hint,
            if is_hover {
                theme::accent().patch(row_bg)
            } else {
                theme::faint().patch(row_bg)
            },
        ));
    } else if is_hover {
        let pad = total_w.saturating_sub(left_used);
        if pad > 0 {
            header_spans.push(Span::styled(" ".repeat(pad), row_bg));
        }
    }

    let mut lines = Vec::with_capacity(usize::from(area.height));
    lines.push(Line::from(header_spans));

    // 2. 展开态渲染精简步骤列表（精确匹配可用高度，绝不静默丢项）
    if expanded && area.height > 1 {
        let body_cap = usize::from(area.height.saturating_sub(1));
        let win = compute_expanded_window(items, body_cap);
        let text_room = area.width.saturating_sub(4).max(4);
        if win.show_above {
            lines.push(Line::styled(
                format!("  … 上方还有 {} 项", win.above),
                theme::faint(),
            ));
        }
        for item in win.items {
            let st = todo::parse_status(&item.status);
            let (sym, sym_style, text_style) =
                todo::status_visual(st, busy && st == TodoStatus::InProgress, tick);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{sym} "), sym_style),
                Span::styled(linebreak::clip(&item.content, text_room), text_style),
            ]));
        }
        if win.show_below {
            let omitted = if !win.show_above && win.above > 0 {
                win.above + win.below
            } else {
                win.below
            };
            lines.push(Line::styled(
                format!("  … 其余 {omitted} 项"),
                theme::faint(),
            ));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
    Rect::new(area.x, area.y, area.width, 1)
}
