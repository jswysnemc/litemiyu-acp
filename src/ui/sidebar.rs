//! 右侧边栏（参考 LiteMiyu `ui/sidebar.rs`）：
//! 会话信息、工作区、上下文进度条、用量统计、跳转消息、最近会话。

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::acp::SessionSummary;
use crate::background::BackgroundTask;
use crate::linebreak;
use crate::theme;
use crate::todo::{self, TodoStatus};
use crate::transcript::{Transcript, format_short, short_id};
use crate::ui::HoverTarget;
use crate::ui::activity;
use crate::ui::overlay::format_compact_time;

/// 后台任务旋转指示符（4 帧顺时针几何圆点，避免 Emoji）。
const BG_SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

#[derive(Default, Debug, Clone)]
pub struct SidebarHits {
    pub area: Rect,
    pub context_area: Rect,
    pub todo_area: Rect,
    pub todo_entry_id: Option<u64>,
    pub bg_rows: Vec<(u16, u64)>,         // (screen_y, entry_id)
    pub subagent_rows: Vec<(u16, u64)>,
    pub jump_rows: Vec<(u16, u64)>,       // (screen_y, entry_id)
    pub session_rows: Vec<(u16, String)>, // (screen_y, session_id)
}

pub struct SidebarData<'a> {
    pub transcript: &'a Transcript,
    pub cwd: &'a str,
    pub mode_label: &'a str,
    pub tier: Option<&'a str>,
    pub busy: bool,
    pub tick: usize,
    pub current_user_entry: Option<u64>,
    pub background_tasks: &'a [BackgroundTask],
    pub subagents: &'a [crate::subagent::Agent],
    pub started_at: Instant,
    pub recent_sessions: &'a [SessionSummary],
    pub hover: &'a HoverTarget,
}

pub fn draw(frame: &mut Frame, area: Rect, data: &SidebarData<'_>) -> SidebarHits {
    let mut hits = SidebarHits {
        area,
        ..SidebarHits::default()
    };
    if area.width < 12 || area.height < 4 {
        return hits;
    }

    // 左侧柔和竖线分隔符
    let sep_lines: Vec<Line<'static>> = (0..area.height)
        .map(|_| Line::styled("│", theme::faint()))
        .collect();
    frame.render_widget(
        Paragraph::new(sep_lines),
        Rect::new(area.x, area.y, 1, area.height),
    );

    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(2),
    );
    let w = inner.width;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut bg_marks: Vec<(usize, u64)> = Vec::new();
    let mut subagent_marks = Vec::new();
    let mut jump_marks: Vec<(usize, u64)> = Vec::new();
    let mut session_marks: Vec<(usize, String)> = Vec::new();

    // 1. 会话
    let title = data
        .transcript
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("未命名会话");
    lines.push(Line::styled(linebreak::clip(title, w), theme::side_title()));
    if let Some(sid) = &data.transcript.session_id {
        lines.push(Line::styled(format!("  #{}", short_id(sid)), theme::dim()));
    }
    let mut meta = format!("  模式 {}", data.mode_label);
    if let Some(tier) = data.tier {
        meta.push_str(&format!(" · {tier}"));
    }
    lines.push(Line::styled(linebreak::clip(&meta, w), theme::faint()));

    // 2. 工作区
    lines.push(Line::default());
    lines.push(Line::styled("工作区", theme::side_title()));
    for part in wrap_path(data.cwd, usize::from(w.saturating_sub(2)).max(1)) {
        lines.push(Line::styled(format!("  {part}"), theme::dim()));
    }

    // 2.5 后台运行任务（仅存在运行中后台任务时紧凑展示，点击定位对应工具行）
    let hover_bg = theme::palette().row_focus_bg;
    if !data.subagents.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(format!("子代理 ({})", data.subagents.len()), theme::side_title()));
        let mut recent: Vec<_> = data.subagents.iter().rev().collect();
        recent.sort_by_key(|agent| matches!(agent.phase, crate::subagent::Phase::Completed | crate::subagent::Phase::Failed));
        for agent in recent.into_iter().take(3) {
            subagent_marks.push((lines.len(), agent.entry_id));
            let hovered = *data.hover == HoverTarget::SidebarSubagent(agent.entry_id);
            let label = format!("{} {}", agent.headline(), agent.elapsed_label());
            let style = if hovered { theme::accent_bold().bg(hover_bg) } else { theme::dim() };
            lines.push(pad_hover_line(vec![Span::styled(linebreak::clip(label.trim_end(), w), style)], w, hovered, hover_bg));
        }
    }
    if !data.background_tasks.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("后台任务", theme::side_title()),
            Span::styled(
                format!(" ({})", data.background_tasks.len()),
                theme::accent(),
            ),
        ]));
        let spin_idx = ((data.started_at.elapsed().as_millis() / 250) as usize) % BG_SPINNER.len();
        let spinner = BG_SPINNER[spin_idx];
        let content_room = usize::from(w.saturating_sub(2));
        for task in data.background_tasks.iter().take(3) {
            let row_idx = lines.len();
            bg_marks.push((row_idx, task.entry_id));
            let is_hover =
                matches!(data.hover, HoverTarget::SidebarBackground(id) if *id == task.entry_id);
            let row_bg = if is_hover {
                Style::new().bg(hover_bg)
            } else {
                Style::new()
            };
            let lead = Span::styled(format!("{spinner} "), theme::accent().patch(row_bg));
            let text_style = if is_hover {
                theme::text().add_modifier(Modifier::BOLD).patch(row_bg)
            } else {
                theme::text()
            };
            let elapsed_str = activity::format_elapsed(task.elapsed());
            let elapsed_w = elapsed_str.width();
            let elapsed_style = if is_hover {
                theme::dim().patch(row_bg)
            } else {
                theme::faint()
            };
            let spans = if content_room > elapsed_w + 6 {
                let title_room = (content_room - elapsed_w - 1) as u16;
                let clipped = linebreak::clip(&task.label, title_room);
                let pad = content_room
                    .saturating_sub(clipped.width() + elapsed_w)
                    .max(1);
                vec![
                    lead,
                    Span::styled(clipped, text_style),
                    Span::styled(" ".repeat(pad), row_bg),
                    Span::styled(elapsed_str, elapsed_style),
                ]
            } else {
                vec![
                    lead,
                    Span::styled(linebreak::clip(&task.label, w.saturating_sub(2)), text_style),
                ]
            };
            lines.push(pad_hover_line(spans, w, is_hover, hover_bg));
        }
        if data.background_tasks.len() > 3 {
            lines.push(Line::styled(
                format!("  +{} 项后台运行中", data.background_tasks.len() - 3),
                theme::faint(),
            ));
        }
    }

    // 2.6 任务清单（实时跟踪 agy `update_steps` 与标准 ACP `plan`，点击定位到时间线任务卡片）
    let mut todo_rows: Option<(usize, usize)> = None;
    if !data.transcript.latest_plan.is_empty() {
        lines.push(Line::default());
        let todo_start = lines.len();
        let todo_hover = matches!(data.hover, HoverTarget::SidebarTodo);
        let row_bg = if todo_hover {
            Style::new().bg(hover_bg)
        } else {
            Style::new()
        };
        let counts = todo::count_items(&data.transcript.latest_plan);
        let all_done = counts.all_done();
        let title_style = if todo_hover {
            theme::accent_bold().patch(row_bg)
        } else if all_done {
            theme::good()
        } else {
            theme::side_title()
        };
        let count_style = if all_done {
            theme::good().patch(row_bg)
        } else if counts.failed > 0 {
            theme::error().patch(row_bg)
        } else {
            theme::accent().patch(row_bg)
        };
        lines.push(pad_hover_line(
            vec![
                Span::styled("任务清单", title_style),
                Span::styled(
                    format!(" ({}/{})", counts.finished(), counts.total),
                    count_style,
                ),
            ],
            w,
            todo_hover,
            hover_bg,
        ));
        let (window, above, below) = todo::visible_window(&data.transcript.latest_plan, 6);
        let text_room = w.saturating_sub(4).max(4);
        if above > 0 {
            lines.push(pad_hover_line(
                vec![Span::styled(
                    format!("  … 上方 {above} 项"),
                    theme::faint().patch(row_bg),
                )],
                w,
                todo_hover,
                hover_bg,
            ));
        }
        for item in window {
            let st = todo::parse_status(&item.status);
            let (sym, sym_style, text_style) =
                todo::status_visual(st, data.busy && st == TodoStatus::InProgress, data.tick);
            let shown_text_style = if todo_hover {
                theme::text().patch(row_bg)
            } else {
                text_style.patch(row_bg)
            };
            lines.push(pad_hover_line(
                vec![
                    Span::styled("  ", row_bg),
                    Span::styled(format!("{sym} "), sym_style.patch(row_bg)),
                    Span::styled(linebreak::clip(&item.content, text_room), shown_text_style),
                ],
                w,
                todo_hover,
                hover_bg,
            ));
        }
        if below > 0 {
            lines.push(pad_hover_line(
                vec![Span::styled(
                    format!("  … 其余 {below} 项"),
                    theme::faint().patch(row_bg),
                )],
                w,
                todo_hover,
                hover_bg,
            ));
        }
        todo_rows = Some((todo_start, lines.len()));
    }

    // 3. 上下文（整段可点击查看状态/触发压缩）
    let mut context_rows: Option<(usize, usize)> = None;
    if data.transcript.context_window > 0 {
        lines.push(Line::default());
        let ctx_start = lines.len();
        let ctx_hover = matches!(data.hover, HoverTarget::SidebarContext);
        let ctx_title_style = if ctx_hover {
            theme::accent_bold().bg(hover_bg)
        } else {
            theme::side_title()
        };
        let ctx_meta_style = if ctx_hover {
            theme::text().bg(hover_bg)
        } else {
            theme::dim()
        };
        lines.push(pad_hover_line(
            vec![Span::styled("上下文", ctx_title_style)],
            w,
            ctx_hover,
            hover_bg,
        ));
        let used = data.transcript.context_used;
        let win = data.transcript.context_window;
        let pct = (used as f64 / win.max(1) as f64) * 100.0;
        lines.push(pad_hover_line(
            vec![Span::styled(
                format!(
                    "  {} / {} · {:.1}%",
                    format_short(used),
                    format_short(win),
                    pct
                ),
                ctx_meta_style,
            )],
            w,
            ctx_hover,
            hover_bg,
        ));
        let bar_w = usize::from(w.saturating_sub(2)).clamp(8, 22);
        let raw_filled = ((used as f64 / win.max(1) as f64) * bar_w as f64).round() as usize;
        let filled = if used > 0 { raw_filled.max(1) } else { 0 }.min(bar_w);
        let empty = bar_w.saturating_sub(filled);
        let bar_pad = if ctx_hover {
            Style::new().bg(hover_bg)
        } else {
            Style::new()
        };
        lines.push(pad_hover_line(
            vec![
                Span::styled("  ", bar_pad),
                Span::styled("▰".repeat(filled), theme::accent().patch(bar_pad)),
                Span::styled("▱".repeat(empty), theme::faint().patch(bar_pad)),
            ],
            w,
            ctx_hover,
            hover_bg,
        ));
        context_rows = Some((ctx_start, lines.len()));
    }

    // 4. 用量
    let total_io = data.transcript.total_input + data.transcript.total_output;
    if total_io > 0 {
        lines.push(Line::default());
        lines.push(Line::styled("用量", theme::side_title()));
        lines.push(Line::styled(
            format!("  共 {} token", format_short(total_io + data.transcript.total_cached_read)),
            theme::dim(),
        ));
        lines.push(Line::styled(
            format!(
                "  输入 {} · 输出 {}",
                format_short(data.transcript.total_input),
                format_short(data.transcript.total_output)
            ),
            theme::dim(),
        ));
        let denom = data.transcript.total_input + data.transcript.total_cached_read;
        if denom > 0 && data.transcript.total_cached_read > 0 {
            let hit = (data.transcript.total_cached_read * 100) / denom;
            lines.push(Line::styled(format!("  缓存命中 {hit}%"), theme::dim()));
        }
    }

    // 5. 跳转消息
    let prompts = data.transcript.user_prompts(8);
    if !prompts.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled("跳转消息", theme::side_title()));
        for (eid, text) in prompts {
            let row_idx = lines.len();
            jump_marks.push((row_idx, eid));
            let is_cur = data.current_user_entry == Some(eid);
            let is_hover = matches!(data.hover, HoverTarget::SidebarJump(id) if *id == eid);
            let row_bg = if is_hover {
                Style::new().bg(hover_bg)
            } else {
                Style::new()
            };
            let lead = if is_cur {
                Span::styled("❱ ", theme::accent().patch(row_bg))
            } else if is_hover {
                Span::styled("› ", theme::accent().patch(row_bg))
            } else {
                Span::styled("  ", row_bg)
            };
            let style = if is_hover {
                theme::text().add_modifier(Modifier::BOLD).patch(row_bg)
            } else if is_cur {
                theme::text()
            } else {
                theme::dim()
            };
            lines.push(pad_hover_line(
                vec![
                    lead,
                    Span::styled(linebreak::clip(&text, w.saturating_sub(2)), style),
                ],
                w,
                is_hover,
                hover_bg,
            ));
        }
    }

    // 6. 最近会话
    if !data.recent_sessions.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled("最近会话", theme::side_title()));
        let content_room = usize::from(w.saturating_sub(2));
        for s in data.recent_sessions.iter().take(8) {
            let row_idx = lines.len();
            session_marks.push((row_idx, s.session_id.clone()));
            let is_cur = data.transcript.session_id.as_deref() == Some(s.session_id.as_str());
            let is_hover =
                matches!(data.hover, HoverTarget::SidebarSession(sid) if sid == &s.session_id);
            let row_bg = if is_hover {
                Style::new().bg(hover_bg)
            } else {
                Style::new()
            };
            let lead = if is_cur {
                Span::styled("❱ ", theme::accent().patch(row_bg))
            } else if is_hover {
                Span::styled("› ", theme::accent().patch(row_bg))
            } else {
                Span::styled("  ", row_bg)
            };
            let label = if s.title.trim().is_empty() {
                format!("#{}", short_id(&s.session_id))
            } else {
                s.title.clone()
            };
            let style = if is_hover {
                theme::text().add_modifier(Modifier::BOLD).patch(row_bg)
            } else if is_cur {
                theme::text()
            } else {
                theme::dim()
            };
            let time_style = if is_hover {
                theme::dim().patch(row_bg)
            } else {
                theme::faint()
            };
            let time_str = format_compact_time(&s.updated_at);
            let time_w = time_str.width();

            let spans = if !time_str.is_empty() && content_room > time_w + 6 {
                let title_room = (content_room - time_w - 1) as u16;
                let clipped = linebreak::clip(&label, title_room);
                let pad = content_room.saturating_sub(clipped.width() + time_w).max(1);
                vec![
                    lead,
                    Span::styled(clipped, style),
                    Span::styled(" ".repeat(pad), row_bg),
                    Span::styled(time_str, time_style),
                ]
            } else {
                vec![
                    lead,
                    Span::styled(linebreak::clip(&label, w.saturating_sub(2)), style),
                ]
            };
            lines.push(pad_hover_line(spans, w, is_hover, hover_bg));
        }
    }

    let max_h = usize::from(inner.height);
    hits.subagent_rows.extend(subagent_marks.into_iter().filter(|(row, _)| *row < max_h)
        .map(|(row, entry)| (inner.y + row as u16, entry)));
    if let Some((start, end)) = todo_rows
        && start < max_h
    {
        let shown_end = end.min(max_h);
        hits.todo_area = Rect::new(
            inner.x,
            inner.y + start as u16,
            inner.width,
            (shown_end - start) as u16,
        );
        hits.todo_entry_id = data.transcript.latest_plan_entry_id;
    }
    if let Some((start, end)) = context_rows
        && start < max_h {
            let shown_end = end.min(max_h);
            hits.context_area = Rect::new(
                inner.x,
                inner.y + start as u16,
                inner.width,
                (shown_end - start) as u16,
            );
        }
    for (row_idx, eid) in bg_marks {
        if row_idx < max_h {
            hits.bg_rows.push((inner.y + row_idx as u16, eid));
        }
    }
    for (row_idx, eid) in jump_marks {
        if row_idx < max_h {
            hits.jump_rows.push((inner.y + row_idx as u16, eid));
        }
    }
    for (row_idx, sid) in session_marks {
        if row_idx < max_h {
            hits.session_rows.push((inner.y + row_idx as u16, sid));
        }
    }

    lines.truncate(max_h);
    frame.render_widget(Paragraph::new(lines), inner);
    hits
}

/// 悬停高亮行时补齐右侧底色到侧边栏净宽，使高亮条左右齐整。
///
/// 参数:
/// - `spans`: 行内片段
/// - `width`: 侧边栏净宽
/// - `hovered`: 是否处于悬停态
/// - `bg`: 悬停底色
///
/// 返回:
/// - 构造完毕的 `Line`
fn pad_hover_line(
    mut spans: Vec<Span<'static>>,
    width: u16,
    hovered: bool,
    bg: ratatui::style::Color,
) -> Line<'static> {
    if hovered {
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let pad = usize::from(width).saturating_sub(used);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::new().bg(bg)));
        }
    }
    Line::from(spans)
}

fn wrap_path(path: &str, room: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<&str> = path.graphemes(true).collect();
    while !rest.is_empty() {
        let mut used = 0;
        let mut fit = 0;
        for g in &rest {
            let w = g.width();
            if used + w > room {
                break;
            }
            used += w;
            fit += 1;
        }
        if fit == rest.len() {
            lines.push(rest.concat());
            break;
        }
        let cut = rest[..fit]
            .iter()
            .rposition(|g| *g == "/" || *g == "\\")
            .filter(|at| *at > 0)
            .map_or(fit.max(1), |at| at + 1);
        lines.push(rest[..cut].concat());
        rest.drain(..cut);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
