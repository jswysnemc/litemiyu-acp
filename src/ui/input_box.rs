//! 圆角输入框、底部状态行（流光/Toast/后台任务标记）与垂直滚动条绘制。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::QueuedMode;
use crate::background::BackgroundTask;
use crate::input::{AtomKind, AtomSpan, InputEditor};
use crate::linebreak;
use crate::theme;

use super::{HoverTarget, ViewState, activity, input_text_width};

/// 输入框边框与占位文案所需的附加上下文状态。
pub(super) struct InputBoxStatus<'a> {
    pub pending_images: usize,
    pub queued_mode: QueuedMode,
    pub queued_count: usize,
    pub queued_head_preview: Option<&'a str>,
    pub busy: bool,
}

pub(super) fn draw_input_box(
    frame: &mut Frame,
    area: Rect,
    editor: &InputEditor,
    show_cursor: bool,
    is_home: bool,
    status: InputBoxStatus<'_>,
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
        let label = if status.queued_count > 0 {
            format!(" 第 {}/{total_lines} 行 · 队列 {} 条 ", cur_row + 1, status.queued_count)
        } else {
            format!(" 第 {}/{total_lines} 行 ", cur_row + 1)
        };
        block = block.title_top(Line::styled(label, theme::dim()).left_aligned());
    } else if status.queued_count > 0 {
        let preview_budget = area.width.saturating_sub(28).clamp(8, 36);
        let head = status
            .queued_head_preview
            .filter(|s| !s.is_empty())
            .map(|s| format!(": {}", linebreak::clip(s, preview_budget)))
            .unwrap_or_default();
        block = block.title_top(
            Line::styled(
                format!(" 待发 {} 条{head} ", status.queued_count),
                theme::accent(),
            )
            .left_aligned(),
        );
    }

    if status.pending_images > 0 {
        block = block.title_top(
            Line::styled(
                format!(" 已附 {} 张图片 ", status.pending_images),
                theme::md_image(),
            )
            .right_aligned(),
        );
    } else if status.queued_count > 1 && area.width >= 56 {
        block = block.title_top(
            Line::styled(" Ctrl+S 立即插话 · ↑ 撤回 ", theme::dim()).right_aligned(),
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
        } else if status.queued_count > 0 {
            "队列中有待发消息 · ↑ 撤回编辑 · Ctrl+S 立即插话 · /queue 管理"
        } else if status.busy {
            match status.queued_mode {
                QueuedMode::SendImmediately => {
                    "即时插话模式：Enter 直接并发发送 · /queue queue 切回排队"
                }
                QueuedMode::Queue => "生成中：Enter 加入待发队列 · Ctrl+S 立即并发插话 · Esc 中断",
            }
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
    let active_agents = view.subagents.iter().filter(|agent| matches!(agent.phase,
        crate::subagent::Phase::Launching | crate::subagent::Phase::Running)).count();
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
        if active_agents > 0 {
            if !detail.is_empty() { detail.push_str(" · "); }
            detail.push_str(&format!("子代理 {active_agents}"));
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
        } else if active_agents > 0 {
            frame.render_widget(Paragraph::new(Line::styled(format!("子代理 {active_agents}"), theme::accent())), left_area);
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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use crate::app::{App, QueuedMode};
    use crate::theme::{self, ThemePreset};

    fn buffer_text(buf: &ratatui::buffer::Buffer, width: u16, height: u16) -> String {
        let mut out = String::new();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let sym = buf[(x, y)].symbol();
                out.push_str(sym);
                x += u16::try_from(sym.width()).unwrap_or(1).max(1);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn queued_message_and_steer_mode_render_in_input_box_and_footer() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, "/tmp/demo".into(), None, false, true);
        app.connecting = false;
        app.transcript.session_id = Some("sess-1".into());
        app.busy = true;
        app.queued_mode = QueuedMode::Queue;
        app.submit_text("后续再排查测试".into());

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let single_queued_screen = buffer_text(terminal.backend().buffer(), 100, 30);
        assert!(
            single_queued_screen.contains("待发 1 条: 后续再排查测试"),
            "input border should show queued preview:\n{single_queued_screen}"
        );
        assert!(
            !single_queued_screen.contains("Ctrl+S 立即插话 · ↑ 撤回"),
            "single queued message should not show top-right hint:\n{single_queued_screen}"
        );
        assert!(
            single_queued_screen.contains("队列 1"),
            "footer should show queued count:\n{single_queued_screen}"
        );

        // 存在多条队列时，输入框右上角才展示快捷操作提示
        app.submit_text("第二条排队消息".into());
        terminal.draw(|f| app.render(f)).unwrap();
        let multi_queued_screen = buffer_text(terminal.backend().buffer(), 100, 30);
        assert!(
            multi_queued_screen.contains("待发 2 条: 后续再排查测试"),
            "input border should update queued count:\n{multi_queued_screen}"
        );
        assert!(
            multi_queued_screen.contains("Ctrl+S 立即插话 · ↑ 撤回"),
            "multiple queued messages should show top-right hint:\n{multi_queued_screen}"
        );

        app.queued_mode = QueuedMode::SendImmediately;
        terminal.draw(|f| app.render(f)).unwrap();
        let steer_screen = buffer_text(terminal.backend().buffer(), 100, 30);
        assert!(
            steer_screen.contains("即时插话"),
            "footer should show steer mode badge:\n{steer_screen}"
        );
    }
}
