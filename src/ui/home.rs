//! 启动首页渲染模块：响应式头部状态栏、圆角输入框、可点击配置/快捷项与最近会话直达列表。

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::acp::SessionSummary;
use crate::app::QueuedMode;
use crate::linebreak;
use crate::theme;
use crate::transcript::short_id;

use super::format::{format_compact_time, same_directory, shorten_path_left};
use super::input_box::{InputBoxStatus, draw_input_box};
use super::selection::{ScreenSnapshot, apply_selection_highlight};
use super::{
    FrameHits, HOME_INPUT_MAX_ROWS, HoverTarget, OverlayHits, ViewState, activity, elicitation,
    input_text_width, overlay,
};

/// 启动首页可点击快捷操作标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeAction {
    Model,
    Effort,
    Mode,
    Resume,
    Theme,
    Help,
}

/// 启动首页交互热区集合。
#[derive(Default, Debug, Clone)]
pub struct HomeHits {
    pub actions: Vec<(Rect, HomeAction)>,
    pub sessions: Vec<(u16, Rect, String)>,
}

/// 【启动首页】【绘制布局】渲染居中响应式首页，保证所有提示完整不截断并输出点击热区。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `full`: 终端整屏矩形
/// - `view`: 视图只读状态快照
///
/// 返回:
/// - 包含输入框尺寸、快捷项热区与屏幕快照的 `FrameHits`
pub(super) fn draw(frame: &mut Frame, full: Rect, view: &ViewState<'_>) -> FrameHits {
    let box_w = if full.width >= 36 {
        full.width.saturating_sub(6).clamp(30, 86)
    } else {
        full.width.saturating_sub(2).max(20)
    };
    let inner_w = box_w.saturating_sub(2).max(16);
    let text_w = input_text_width(box_w);
    let max_inner = full
        .height
        .saturating_sub(11)
        .clamp(1, HOME_INPUT_MAX_ROWS);
    let inner_rows = (view.editor.visual_lines(text_w).len() as u16).clamp(1, max_inner);
    let box_h = inner_rows + 2;

    // 1. 预先计算快捷提示行数与最近会话预览行数，确保整体垂直光学居中且绝不越界
    let shortcut_chips = build_shortcut_chips();
    let shortcut_rows = pack_shortcut_rows(&shortcut_chips, usize::from(inner_w));
    let hint_rows_h = if view.toast.is_some() {
        1u16
    } else {
        shortcut_rows.len().clamp(1, 2) as u16
    };

    let (recent_title, recent_list) = select_home_sessions(view.recent_sessions, view.cwd);
    let header_h: u16 = if full.height >= 14 { 3 } else { 2 };
    let base_h = header_h + box_h + 1 + 1 + hint_rows_h;
    let show_recent = !recent_list.is_empty()
        && view.permission.is_none()
        && view.elicitation.is_none()
        && full.height >= base_h + 3 + recent_list.len() as u16;
    let recent_h = if show_recent {
        2 + recent_list.len() as u16
    } else {
        0
    };
    let total_h = base_h + recent_h;

    let box_x = full.x + (full.width.saturating_sub(box_w)) / 2;
    let inner_x = box_x + 1;
    // 光学微偏上（约 40% 留白），同时保证输入框上方保留足够高度给补全菜单
    let top_slack = full.height.saturating_sub(total_h);
    let top_y = full.y + ((u32::from(top_slack) * 2) / 5) as u16;
    let box_y = top_y.saturating_add(header_h);
    let box_area = Rect::new(box_x, box_y, box_w, box_h);

    let mut home_hits = HomeHits::default();

    // 2. 顶部标题与工作区信息栏
    draw_header(frame, Rect::new(inner_x, top_y, inner_w, header_h), view);

    // 3. 中央圆角输入框（或权限/提问抽屉）
    let mut overlay_hits = OverlayHits::default();
    let input_text_area = if let Some((req, sel)) = view.permission {
        let dh = (req.options.len() as u16 + 5).clamp(7, 14);
        overlay_hits.permission = overlay::draw_permission_drawer(
            frame,
            Rect::new(box_x, box_y, box_w, dh),
            req,
            sel,
            view.hover,
        );
        Rect::default()
    } else if let Some(el) = view.elicitation {
        let dh = elicitation::drawer_height(el, full.height);
        overlay_hits.elicitation = elicitation::draw_elicitation_drawer(
            frame,
            Rect::new(box_x, box_y, box_w, dh),
            el,
            view.hover,
        );
        Rect::default()
    } else {
        draw_input_box(
            frame,
            box_area,
            view.editor,
            view.picker.is_none(),
            true,
            InputBoxStatus {
                pending_images: view.pending_images,
                queued_mode: view.queued_mode,
                queued_count: view.queued_count,
                queued_head_preview: view.queued_head_preview,
                busy: view.busy,
            },
        )
    };

    // 4. 输入框下方交互式模型/思考等级/模式配置行
    let meta_y = box_area.bottom();
    if meta_y < full.bottom() {
        draw_meta_row(
            frame,
            Rect::new(inner_x, meta_y, inner_w, 1),
            view,
            &mut home_hits,
        );
    }

    // 5. 自适应换行快捷命令栏（或 Toast 提示），彻底消除右侧文字截断
    let hint_y = meta_y.saturating_add(2);
    if hint_y < full.bottom() {
        if let Some((msg, is_err)) = view.toast {
            let style = if is_err { theme::error() } else { theme::good() };
            frame.render_widget(
                Paragraph::new(Line::styled(linebreak::clip(msg, inner_w), style)),
                Rect::new(inner_x, hint_y, inner_w, 1),
            );
        } else {
            draw_shortcut_rows(
                frame,
                inner_x,
                hint_y,
                inner_w,
                full.bottom(),
                &shortcut_rows,
                view.hover,
                &mut home_hits,
            );
        }
    }

    // 6. 空间充足时展示紧凑最近会话直达列表（最多 3 条，支持悬停高亮与单击恢复）
    if show_recent {
        let recent_y = hint_y.saturating_add(hint_rows_h).saturating_add(1);
        draw_recent_sessions(
            frame,
            Rect::new(inner_x, recent_y, inner_w, full.bottom().saturating_sub(recent_y)),
            recent_title,
            &recent_list,
            view.recent_sessions.len(),
            view.hover,
            &mut home_hits,
        );
    }

    // 7. 输入框悬浮补全菜单与模态选择器
    if view.permission.is_none()
        && view.elicitation.is_none()
        && view.picker.is_none()
        && let Some(menu) = view.completion_menu
    {
        overlay_hits.completion = overlay::draw_completion_menu(
            frame,
            box_area,
            menu,
            view.completion_selected,
            view.hover,
        );
    }
    if let Some(picker) = view.picker {
        overlay_hits.picker = overlay::draw_picker(frame, full, picker, view.hover);
    }

    let screen_snapshot = ScreenSnapshot::capture(frame.buffer_mut(), full);
    let empty_meta = HashMap::new();
    if let Some(sel) = view.selection {
        apply_selection_highlight(frame.buffer_mut(), full, sel, Rect::default(), &empty_meta);
    }

    FrameHits {
        main_area: full,
        input_area: box_area,
        input_text_area,
        input_text_width: text_w,
        home: home_hits,
        overlay: overlay_hits,
        screen_snapshot,
        ..FrameHits::default()
    }
}

/// 绘制首页顶部品牌标识、ACP 连接状态与精简工作区路径行。
fn draw_header(frame: &mut Frame, area: Rect, view: &ViewState<'_>) {
    if area.height == 0 || area.width < 12 {
        return;
    }
    let row0 = Rect::new(area.x, area.y, area.width, 1);
    if view.connecting {
        let anim_frame = activity::frame_at(view.started_at.elapsed());
        frame.render_widget(
            Paragraph::new(activity::render_status_line(
                "正在连接 ACP 服务…",
                "",
                anim_frame,
            )),
            row0,
        );
    } else {
        let ver = format!(" v{}", env!("CARGO_PKG_VERSION"));
        let left_spans = vec![
            Span::styled("◈ ", theme::accent()),
            Span::styled("agy-tui", theme::accent_bold()),
            Span::styled(ver.clone(), theme::faint()),
        ];
        let left_w = 2 + "agy-tui".width() + ver.width();
        frame.render_widget(Paragraph::new(Line::from(left_spans)), row0);

        let right_room = usize::from(area.width).saturating_sub(left_w + 3);
        if right_room >= 8 && !view.agent_banner.trim().is_empty() {
            let banner = linebreak::clip(view.agent_banner, right_room.saturating_sub(2) as u16);
            let right_line = Line::from(vec![
                Span::styled("● ", theme::good()),
                Span::styled(banner, theme::dim()),
            ])
            .right_aligned();
            frame.render_widget(Paragraph::new(right_line), row0);
        }
    }

    // 第 2 行：工作区路径（左）+ 账号等级或当前主题标识（右）
    if area.height >= 3 {
        let row1 = Rect::new(area.x, area.y + 1, area.width, 1);
        let right_meta = view
            .tier
            .filter(|t| !t.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(|| {
                let preset = theme::current_theme_id();
                if theme::paint_bg() {
                    format!("主题 {preset}")
                } else {
                    format!("透明 · {preset}")
                }
            });
        let right_w = right_meta.width();
        let show_right = usize::from(area.width) >= right_w + 22;
        let branch_label = view
            .git_branch
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| linebreak::clip(b, 20));
        let branch_w = branch_label.as_ref().map_or(0, |b| b.width() + 3);
        let base_room = if show_right {
            usize::from(area.width).saturating_sub(right_w + 8)
        } else {
            usize::from(area.width).saturating_sub(5)
        };
        let show_branch = branch_w > 0 && base_room >= branch_w + 8;
        let path_room = if show_branch {
            base_room.saturating_sub(branch_w)
        } else {
            base_room
        };
        let collapsed = crate::paths::collapse_home_dir(view.cwd);
        let short_cwd = shorten_path_left(&collapsed, path_room.max(6) as u16);
        let mut left_spans = vec![
            Span::styled("目录 ", theme::faint()),
            Span::styled(short_cwd, theme::md_url()),
        ];
        if show_branch
            && let Some(branch) = branch_label
        {
            left_spans.push(Span::styled(" (", theme::faint()));
            left_spans.push(Span::styled(branch, Style::new().fg(theme::palette().picked)));
            left_spans.push(Span::styled(")", theme::faint()));
        }
        frame.render_widget(Paragraph::new(Line::from(left_spans)), row1);
        if show_right {
            frame.render_widget(
                Paragraph::new(Line::styled(right_meta, theme::faint()).right_aligned()),
                row1,
            );
        }
    }
}

/// 绘制输入框下方模型/思考等级/模式可点击控制条与输入键提示。
fn draw_meta_row(frame: &mut Frame, area: Rect, view: &ViewState<'_>, hits: &mut HomeHits) {
    let hover_bg = theme::palette().row_focus_bg;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor_x = area.x;

    let mut push_chip = |label: &str, base: Style, action: HomeAction| {
        if label.is_empty() {
            return;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::faint()));
            cursor_x = cursor_x.saturating_add(3);
        }
        let w = u16::try_from(label.width()).unwrap_or(0);
        let idx = hits.actions.len();
        let hovered = matches!(view.hover, HoverTarget::HomeAction(i, a) if *i == idx && *a == action);
        let style = if hovered {
            base.bg(hover_bg).add_modifier(Modifier::UNDERLINED)
        } else {
            base
        };
        if cursor_x < area.right() && w > 0 {
            let clipped_w = w.min(area.right().saturating_sub(cursor_x));
            hits.actions
                .push((Rect::new(cursor_x, area.y, clipped_w, 1), action));
        }
        spans.push(Span::styled(label.to_string(), style));
        cursor_x = cursor_x.saturating_add(w);
    };

    push_chip(view.model_label, theme::model(), HomeAction::Model);
    push_chip(view.effort_label, theme::accent(), HomeAction::Effort);
    push_chip(view.mode_label, theme::dim(), HomeAction::Mode);

    if view.queued_mode == QueuedMode::SendImmediately {
        spans.push(Span::styled(" · ", theme::faint()));
        let q_label = view.queued_mode.short_label();
        spans.push(Span::styled(q_label.to_string(), theme::accent()));
        cursor_x = cursor_x.saturating_add(3 + u16::try_from(q_label.width()).unwrap_or(0));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    let left_used = usize::from(cursor_x.saturating_sub(area.x));
    let avail = usize::from(area.width).saturating_sub(left_used + 2);
    let right_hint = if avail >= "Enter 发送 · Shift+Enter 换行".width() {
        Some("Enter 发送 · Shift+Enter 换行")
    } else if avail >= "Enter 发送".width() {
        Some("Enter 发送")
    } else {
        None
    };
    if let Some(hint) = right_hint {
        frame.render_widget(
            Paragraph::new(Line::styled(hint, theme::faint()).right_aligned()),
            area,
        );
    }
}

/// 首页底部可点击快捷命令条目定义。
#[derive(Clone, Copy)]
struct ShortcutChip {
    key: &'static str,
    label: &'static str,
    action: HomeAction,
}

impl ShortcutChip {
    fn width(self) -> usize {
        self.key.width() + 1 + self.label.width()
    }
}

fn build_shortcut_chips() -> [ShortcutChip; 5] {
    [
        ShortcutChip {
            key: "/resume",
            label: "恢复会话",
            action: HomeAction::Resume,
        },
        ShortcutChip {
            key: "/model",
            label: "模型",
            action: HomeAction::Model,
        },
        ShortcutChip {
            key: "/theme",
            label: "主题",
            action: HomeAction::Theme,
        },
        ShortcutChip {
            key: "Shift+Tab",
            label: "模式",
            action: HomeAction::Mode,
        },
        ShortcutChip {
            key: "/help",
            label: "帮助",
            action: HomeAction::Help,
        },
    ]
}

/// 按可用列宽贪心分行，确保任何终端宽度下都不截断快捷提示文字。
fn pack_shortcut_rows(chips: &[ShortcutChip], max_width: usize) -> Vec<Vec<ShortcutChip>> {
    let mut rows: Vec<Vec<ShortcutChip>> = Vec::new();
    let mut current: Vec<ShortcutChip> = Vec::new();
    let mut used = 0usize;

    for &chip in chips {
        let w = chip.width();
        let needed = if current.is_empty() { w } else { 3 + w };
        if !current.is_empty() && used + needed > max_width {
            rows.push(std::mem::take(&mut current));
            used = 0;
        }
        used += if current.is_empty() { w } else { 3 + w };
        current.push(chip);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn draw_shortcut_rows(
    frame: &mut Frame,
    x: u16,
    start_y: u16,
    width: u16,
    max_bottom: u16,
    rows: &[Vec<ShortcutChip>],
    hover: &HoverTarget,
    hits: &mut HomeHits,
) {
    let hover_bg = theme::palette().row_focus_bg;
    for (row_idx, chips) in rows.iter().enumerate() {
        let y = start_y.saturating_add(row_idx as u16);
        if y >= max_bottom {
            break;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cursor_x = x;
        for (idx, chip) in chips.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::styled(" · ", theme::faint()));
                cursor_x = cursor_x.saturating_add(3);
            }
            let hit_idx = hits.actions.len();
            let hovered = matches!(hover, HoverTarget::HomeAction(i, a) if *i == hit_idx && *a == chip.action);
            let bg = if hovered {
                Style::new().bg(hover_bg)
            } else {
                Style::new()
            };
            let key_style = if hovered {
                theme::accent_bold().patch(bg)
            } else {
                theme::accent().patch(bg)
            };
            let lbl_style = if hovered {
                theme::text().patch(bg)
            } else {
                theme::dim().patch(bg)
            };
            let w = u16::try_from(chip.width()).unwrap_or(0);
            if cursor_x < x.saturating_add(width) && w > 0 {
                let hit_w = w.min(x.saturating_add(width).saturating_sub(cursor_x));
                hits.actions.push((Rect::new(cursor_x, y, hit_w, 1), chip.action));
            }
            spans.push(Span::styled(chip.key, key_style));
            spans.push(Span::styled(" ", bg));
            spans.push(Span::styled(chip.label, lbl_style));
            cursor_x = cursor_x.saturating_add(w);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(x, y, width, 1));
    }
}

/// 挑选首页展示的最多 3 条最近会话（优先匹配当前工作目录）。
fn select_home_sessions<'a>(
    sessions: &'a [SessionSummary],
    cwd: &str,
) -> (&'static str, Vec<&'a SessionSummary>) {
    let same_dir: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| same_directory(&s.cwd, cwd))
        .take(3)
        .collect();
    if !same_dir.is_empty() {
        return ("最近会话 · 当前目录", same_dir);
    }
    ("最近会话", sessions.iter().take(3).collect())
}

fn draw_recent_sessions(
    frame: &mut Frame,
    area: Rect,
    heading: &str,
    sessions: &[&SessionSummary],
    total_sessions: usize,
    hover: &HoverTarget,
    hits: &mut HomeHits,
) {
    if area.height < 2 || area.width < 20 || sessions.is_empty() {
        return;
    }
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let right_meta = format!("共 {total_sessions} 条 · 点击恢复");
    frame.render_widget(
        Paragraph::new(Line::styled(heading.to_string(), theme::side_title())),
        header_area,
    );
    if usize::from(area.width) > heading.width() + right_meta.width() + 2 {
        frame.render_widget(
            Paragraph::new(Line::styled(right_meta, theme::faint()).right_aligned()),
            header_area,
        );
    }

    let hover_bg = theme::palette().row_focus_bg;
    let max_w = usize::from(area.width);
    for (idx, sess) in sessions.iter().enumerate() {
        let y = area.y + 1 + idx as u16;
        if y >= area.bottom() {
            break;
        }
        let is_hover = matches!(hover, HoverTarget::HomeSession(sid) if sid == &sess.session_id);
        let row_bg = if is_hover {
            Style::new().bg(hover_bg)
        } else {
            Style::new()
        };
        let lead = if is_hover {
            Span::styled("› ", theme::accent_bold().patch(row_bg))
        } else {
            Span::styled("› ", theme::faint().patch(row_bg))
        };
        let label = if sess.title.trim().is_empty() {
            format!("未命名会话 #{}", short_id(&sess.session_id))
        } else {
            sess.title.clone()
        };
        let time_str = format_compact_time(&sess.updated_at);
        let time_w = time_str.width();
        let text_style = if is_hover {
            theme::text().add_modifier(Modifier::BOLD).patch(row_bg)
        } else {
            theme::dim().patch(row_bg)
        };
        let time_style = if is_hover {
            theme::accent().patch(row_bg)
        } else {
            theme::faint().patch(row_bg)
        };

        let spans = if !time_str.is_empty() && max_w > time_w + 8 {
            let title_room = (max_w - 2 - time_w - 2) as u16;
            let clipped = linebreak::clip(&label, title_room);
            let pad = max_w.saturating_sub(2 + clipped.width() + time_w).max(1);
            vec![
                lead,
                Span::styled(clipped, text_style),
                Span::styled(" ".repeat(pad), row_bg),
                Span::styled(time_str, time_style),
            ]
        } else {
            let clipped = linebreak::clip(&label, area.width.saturating_sub(2));
            let mut s = vec![lead, Span::styled(clipped.clone(), text_style)];
            if is_hover {
                let pad = max_w.saturating_sub(2 + clipped.width());
                if pad > 0 {
                    s.push(Span::styled(" ".repeat(pad), row_bg));
                }
            }
            s
        };

        let row_rect = Rect::new(area.x, y, area.width, 1);
        frame.render_widget(Paragraph::new(Line::from(spans)), row_rect);
        hits.sessions.push((y, row_rect, sess.session_id.clone()));
    }
}
