//! 右侧边栏（参考 LiteMiyu `ui/sidebar.rs`）：
//! 会话信息、工作区、上下文进度条、用量统计、跳转消息、最近会话。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::acp::SessionSummary;
use crate::linebreak;
use crate::theme;
use crate::transcript::{Transcript, format_short, short_id};
use crate::ui::overlay::format_compact_time;

#[derive(Default, Debug, Clone)]
pub struct SidebarHits {
    pub area: Rect,
    pub context_area: Rect,
    pub jump_rows: Vec<(u16, u64)>,       // (screen_y, entry_id)
    pub session_rows: Vec<(u16, String)>, // (screen_y, session_id)
}

pub struct SidebarData<'a> {
    pub transcript: &'a Transcript,
    pub cwd: &'a str,
    pub mode_label: &'a str,
    pub tier: Option<&'a str>,
    pub current_user_entry: Option<u64>,
    pub recent_sessions: &'a [SessionSummary],
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

    // 3. 上下文（整段可点击查看状态/触发压缩）
    let mut context_rows: Option<(usize, usize)> = None;
    if data.transcript.context_window > 0 {
        lines.push(Line::default());
        let ctx_start = lines.len();
        lines.push(Line::styled("上下文", theme::side_title()));
        let used = data.transcript.context_used;
        let win = data.transcript.context_window;
        let pct = (used as f64 / win.max(1) as f64) * 100.0;
        lines.push(Line::styled(
            format!(
                "  {} / {} · {:.1}%",
                format_short(used),
                format_short(win),
                pct
            ),
            theme::dim(),
        ));
        let bar_w = usize::from(w.saturating_sub(2)).min(22).max(8);
        let raw_filled = ((used as f64 / win.max(1) as f64) * bar_w as f64).round() as usize;
        let filled = if used > 0 { raw_filled.max(1) } else { 0 }.min(bar_w);
        let empty = bar_w.saturating_sub(filled);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("▰".repeat(filled), theme::accent()),
            Span::styled("▱".repeat(empty), theme::faint()),
        ]));
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
            let lead = if is_cur {
                Span::styled("❱ ", theme::accent())
            } else {
                Span::raw("  ")
            };
            let style = if is_cur { theme::text() } else { theme::dim() };
            lines.push(Line::from(vec![
                lead,
                Span::styled(linebreak::clip(&text, w.saturating_sub(2)), style),
            ]));
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
            let lead = if is_cur {
                Span::styled("❱ ", theme::accent())
            } else {
                Span::raw("  ")
            };
            let label = if s.title.trim().is_empty() {
                format!("#{}", short_id(&s.session_id))
            } else {
                s.title.clone()
            };
            let style = if is_cur { theme::text() } else { theme::dim() };
            let time_str = format_compact_time(&s.updated_at);
            let time_w = time_str.width();

            if !time_str.is_empty() && content_room > time_w + 6 {
                let title_room = (content_room - time_w - 1) as u16;
                let clipped = linebreak::clip(&label, title_room);
                let pad = content_room.saturating_sub(clipped.width() + time_w).max(1);
                lines.push(Line::from(vec![
                    lead,
                    Span::styled(clipped, style),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(time_str, theme::faint()),
                ]));
            } else {
                lines.push(Line::from(vec![
                    lead,
                    Span::styled(linebreak::clip(&label, w.saturating_sub(2)), style),
                ]));
            }
        }
    }

    let max_h = usize::from(inner.height);
    if let Some((start, end)) = context_rows {
        if start < max_h {
            let shown_end = end.min(max_h);
            hits.context_area = Rect::new(
                inner.x,
                inner.y + start as u16,
                inner.width,
                (shown_end - start) as u16,
            );
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
