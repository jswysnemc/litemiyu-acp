//! 圆角输入框、底部状态行（流光/Toast/后台任务标记）与垂直滚动条绘制。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::background::BackgroundTask;
use crate::input::{AtomKind, AtomSpan, InputEditor};
use crate::linebreak;
use crate::theme;

use super::{HoverTarget, ViewState, activity, input_text_width};

pub(super) fn draw_input_box(
    frame: &mut Frame,
    area: Rect,
    editor: &InputEditor,
    show_cursor: bool,
    is_home: bool,
    pending_images: usize,
) -> Rect {
    let text_w = input_text_width(area.width);
    let visible_h = usize::from(area.height.saturating_sub(2));
    let vlines = if editor.is_empty() {
        Vec::new()
    } else {
        editor.visual_lines(text_w)
    };
    let (cur_row, cur_col) = if editor.is_empty() {
        (0, 0)
    } else {
        editor.cursor_visual(text_w)
    };
    let total_lines = vlines.len();
    let overflowing = total_lines > visible_h && visible_h > 0;

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::faint());
    if overflowing {
        block = block.title_top(
            Line::styled(
                format!(" 第 {}/{total_lines} 行 ", cur_row + 1),
                theme::dim(),
            )
            .left_aligned(),
        );
    }
    if pending_images > 0 {
        block = block.title_top(
            Line::styled(
                format!(" 已附 {pending_images} 张图片 "),
                theme::md_image(),
            )
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height == 0 {
        return Rect::default();
    }

    // 左侧提示符 `❯ `
    frame.render_widget(
        Paragraph::new(Line::styled(" ❯ ", theme::accent_bold())),
        Rect::new(inner.x, inner.y, 3, 1),
    );

    let text_area = Rect::new(
        inner.x + 3,
        inner.y,
        inner.width.saturating_sub(4).max(1),
        inner.height,
    );

    if editor.is_empty() {
        let placeholder = if is_home {
            "/ 命令 · # Skill · @ 文件 · Ctrl+V 贴图 · 拖拽图片直传"
        } else {
            "/ 命令 · # Skill · @ 文件 · Shift+Enter 换行 · Ctrl+O 展开细节"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    linebreak::clip(placeholder, text_area.width.saturating_sub(1)),
                    theme::faint(),
                ),
            ])),
            text_area,
        );
        if show_cursor {
            frame.set_cursor_position((text_area.x, text_area.y));
        }
        return text_area;
    }

    let scroll = if total_lines <= visible_h {
        0
    } else {
        cur_row
            .saturating_sub(visible_h / 2)
            .min(total_lines.saturating_sub(visible_h))
    };

    let text = editor.text();
    let atoms = editor.atom_spans();
    let p = theme::palette();
    let image_chip_style = theme::md_image().bg(p.surface);
    let paste_chip_style = theme::accent().bg(p.surface);
    let lines: Vec<Line<'static>> = vlines
        .iter()
        .skip(scroll)
        .take(visible_h)
        .map(|vl| {
            style_input_line(
                text,
                vl.start,
                vl.end,
                &atoms,
                image_chip_style,
                paste_chip_style,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), text_area);

    // 超过最大可见行数时，在输入框右侧留白列绘制迷你滚动条提示内部滚动位置
    if overflowing && inner.width >= 5 {
        let bar_x = inner.right().saturating_sub(1);
        draw_scrollbar(
            frame,
            Rect::new(bar_x, inner.y, 1, inner.height),
            total_lines,
            visible_h,
            scroll,
        );
    }

    if show_cursor {
        let cy = text_area.y + (cur_row.saturating_sub(scroll)) as u16;
        let cx = text_area.x + cur_col.min(text_area.width.saturating_sub(1));
        if cy < text_area.bottom() {
            frame.set_cursor_position((cx, cy));
        }
    }
    text_area
}

fn style_input_line(
    full_text: &str,
    line_start: usize,
    line_end: usize,
    atoms: &[AtomSpan],
    image_chip_style: Style,
    paste_chip_style: Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    let mut pos = line_start;

    for atom in atoms {
        if atom.end <= line_start {
            continue;
        }
        if atom.start >= line_end {
            break;
        }
        let seg_start = atom.start.max(line_start);
        let seg_end = atom.end.min(line_end);
        if seg_start > pos {
            let plain = full_text[pos..seg_start].replace('\t', "    ");
            spans.push(Span::styled(plain, theme::text()));
        }
        let chip_style = match atom.kind {
            AtomKind::Image => image_chip_style,
            AtomKind::PastedText => paste_chip_style,
        };
        let chip_text = full_text[seg_start..seg_end].replace('\t', "    ");
        spans.push(Span::styled(chip_text, chip_style));
        pos = seg_end;
    }

    if pos < line_end {
        let tail = full_text[pos..line_end].replace('\t', "    ");
        spans.push(Span::styled(tail, theme::text()));
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    Line::from(spans)
}

pub(super) fn draw_pulse_line(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState<'_>,
    scrolled_up: bool,
) -> Rect {
    if view.connecting {
        let anim_frame = activity::frame_at(view.started_at.elapsed());
        frame.render_widget(
            Paragraph::new(activity::render_status_line(
                "正在初始化 ACP 会话…",
                "",
                anim_frame,
            )),
            area,
        );
        return Rect::default();
    }
    if view.busy {
        let mut detail = view
            .transcript
            .turn_started
            .map(|s| activity::format_elapsed(s.elapsed()))
            .unwrap_or_default();
        if !view.background_tasks.is_empty() {
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&format!("后台 {}", view.background_tasks.len()));
        }
        let anim_frame = activity::frame_at(view.started_at.elapsed());
        let label = activity::infer_work_status(view.transcript);
        let status_line = activity::render_status_line(label, &detail, anim_frame);

        let btn_hovered = matches!(view.hover, HoverTarget::BottomButton);
        let (right_line, right_w, btn_hit) = if scrolled_up {
            let btn = "↓ 回到底部";
            let esc = "Esc 中断 · ";
            let rw = u16::try_from(esc.width() + btn.width())
                .unwrap_or(0)
                .min(area.width);
            let btn_w = u16::try_from(btn.width()).unwrap_or(0).min(area.width);
            let btn_style = if btn_hovered {
                theme::picked_bar()
            } else {
                theme::accent_bold()
            };
            let line = Line::from(vec![
                Span::styled(esc, theme::faint()),
                Span::styled(btn, btn_style),
            ])
            .right_aligned();
            let hit = Rect::new(area.right().saturating_sub(btn_w), area.y, btn_w, 1);
            (line, rw, hit)
        } else {
            let esc = "Esc 中断";
            let rw = u16::try_from(esc.width()).unwrap_or(0).min(area.width);
            (Line::styled(esc, theme::faint()).right_aligned(), rw, Rect::default())
        };

        let left_w = area.width.saturating_sub(right_w.saturating_add(1));
        if left_w > 0 {
            frame.render_widget(
                Paragraph::new(status_line),
                Rect::new(area.x, area.y, left_w, 1),
            );
        }
        if right_w > 0 {
            frame.render_widget(
                Paragraph::new(right_line),
                Rect::new(area.right().saturating_sub(right_w), area.y, right_w, 1),
            );
        }
        return btn_hit;
    }
    let mut btn_hit = Rect::default();
    let mut right_reserved = 0u16;
    if scrolled_up {
        let label = " ↓ 回到底部 (End) ";
        let style = if matches!(view.hover, HoverTarget::BottomButton) {
            theme::picked_bar()
        } else {
            theme::accent_bold().bg(theme::palette().surface)
        };
        let hint = Line::styled(label, style).right_aligned();
        frame.render_widget(Paragraph::new(hint), area);
        let btn_w = u16::try_from(label.width()).unwrap_or(0).min(area.width);
        btn_hit = Rect::new(area.right().saturating_sub(btn_w), area.y, btn_w, 1);
        right_reserved = btn_w.saturating_add(1);
    }

    let left_w = area.width.saturating_sub(right_reserved);
    if left_w > 0 {
        let left_area = Rect::new(area.x, area.y, left_w, 1);
        if let Some((msg, is_err)) = view.toast {
            let style = if is_err { theme::error() } else { theme::good() };
            frame.render_widget(
                Paragraph::new(Line::styled(linebreak::clip(msg, left_w), style)),
                left_area,
            );
        } else if !view.background_tasks.is_empty() {
            let line = render_background_badge_line(
                view.background_tasks,
                view.started_at.elapsed(),
                left_w,
            );
            frame.render_widget(Paragraph::new(line), left_area);
        }
    }
    btn_hit
}

/// 构造空闲态状态行左侧的紧凑后台任务运行标记（如 `◐ 后台任务 1 · sleep 6 · 12s`）。
///
/// 参数:
/// - `tasks`: 当前运行中的后台任务列表
/// - `app_elapsed`: 应用启动至今时长（用于驱动 250ms 旋转指示符）
/// - `max_w`: 左侧可用列宽
///
/// 返回:
/// - 适配宽度后的单行 `Line`
fn render_background_badge_line(
    tasks: &[BackgroundTask],
    app_elapsed: std::time::Duration,
    max_w: u16,
) -> Line<'static> {
    const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];
    let spin = SPINNER[((app_elapsed.as_millis() / 250) as usize) % SPINNER.len()];
    let count = tasks.len();
    let head_badge = format!("{spin} 后台任务 {count}");
    let Some(first) = tasks.first() else {
        return Line::styled(linebreak::clip(&head_badge, max_w), theme::accent());
    };
    let elapsed = activity::format_elapsed(first.elapsed());
    let suffix = if count > 1 {
        format!(" 等 · {elapsed}")
    } else {
        format!(" · {elapsed}")
    };
    let head_w = head_badge.width();
    let suffix_w = suffix.width();
    let room = usize::from(max_w);

    // 1. 宽度充足时展示「旋转点 + 数量 + 首项任务摘要 + 耗时」
    if room > head_w + suffix_w + 6 {
        let summary_room = (room - head_w - suffix_w - 3) as u16;
        let summary = linebreak::clip(&first.label, summary_room);
        return Line::from(vec![
            Span::styled(head_badge, theme::accent()),
            Span::styled(" · ", theme::faint()),
            Span::styled(summary, theme::dim()),
            Span::styled(suffix, theme::faint()),
        ]);
    }
    // 2. 窄屏回退为「旋转点 + 数量 + 耗时」
    let compact = format!("{head_badge} · {elapsed}");
    Line::styled(linebreak::clip(&compact, max_w), theme::accent())
}

pub(super) fn draw_scrollbar(frame: &mut Frame, area: Rect, total: usize, visible: usize, scroll: usize) {
    let h = usize::from(area.height);
    if h == 0 || total <= visible {
        return;
    }
    let thumb_h = ((visible * h) / total).clamp(1, h);
    let max_scroll = total - visible;
    let thumb_top = (scroll * (h - thumb_h))
        .checked_div(max_scroll)
        .unwrap_or(0);
    let lines: Vec<Line<'static>> = (0..h)
        .map(|row| {
            if row >= thumb_top && row < thumb_top + thumb_h {
                Line::styled("┃", theme::dim())
            } else {
                Line::styled("│", theme::faint())
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
