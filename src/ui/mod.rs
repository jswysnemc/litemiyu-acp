//! 主 TUI 渲染协调器（复刻 LiteMiyu 首页 + 对话双栏视图 + 流光状态行 + 圆角输入框 + 极简底栏）。

pub mod activity;
mod elicitation;
pub mod format;
mod input_box;
#[cfg(test)]
mod interaction_tests;
pub mod overlay;
pub mod rows;
pub mod selection;
pub mod sidebar;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::acp::{PermissionRequest, SessionSummary};
use crate::app::{ElicitationState, QueuedMode};
use crate::background::BackgroundTask;
use crate::completion::CompletionMenu;
use crate::input::InputEditor;
use crate::linebreak;
use crate::theme;
use crate::transcript::{FoldSection, Transcript, format_short};

/// 对话界面（非开始界面）输入框最大可见行数（内容增加时自动增高，超过后内部滚动）。
pub const CHAT_INPUT_MAX_ROWS: u16 = 8;
/// 居中启动首页输入框最大可见行数。
pub const HOME_INPUT_MAX_ROWS: u16 = 6;

/// 统一计算圆角输入框内文字区的净列宽（扣除左右边框 2 + 左侧 ` ❯ ` 3 + 右侧留白/滚动条 1 = 6）。
pub fn input_text_width(box_w: u16) -> u16 {
    box_w.saturating_sub(6).max(1)
}

use overlay::PickerState;
use rows::{MdCache, RenderRow, RowCopyMeta, build_body, row_to_line};
use input_box::{InputBoxStatus, draw_input_box, draw_pulse_line, draw_scrollbar};
use selection::{ScreenSnapshot, apply_selection_highlight};
pub use selection::TextSelection;
use sidebar::{SidebarData, SidebarHits};

/// 正文单行命中热区元信息（屏幕行、条目 ID、是否可折叠、行内链接列表）。
pub type BodyRowHit = (u16, u64, bool, Vec<(u16, u16, String)>);

/// 紧凑可交互组件的鼠标悬停目标（不含正文大块与折叠区，避免大面积闪烁）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HoverTarget {
    #[default]
    None,
    SidebarBackground(u64),
    SidebarSubagent(u64),
    Fold(u64, FoldSection),
    SidebarJump(u64),
    SidebarSession(String),
    SidebarContext,
    BottomButton,
    UsageCell,
    BodyLink {
        row: u16,
        col_start: u16,
        col_end: u16,
    },
}


#[derive(Default, Debug, Clone)]
pub struct FrameHits {
    /// 主对话栏区域（不含右侧边栏及其分隔竖线）
    pub main_area: Rect,
    pub body: Rect,
    /// 正文区纯文字区域（不含右侧滚动条列）
    pub body_text: Rect,
    /// 正文右侧滚动条点击/拖动热区
    pub scrollbar: Rect,
    /// 正文右侧滚动条装饰列（用于划词复制兜底排除 `┃`/`│`）
    pub scrollbar_col: Option<u16>,
    /// 右侧边栏左侧分隔竖线列（用于划词复制兜底排除 `│`）
    pub sidebar_divider_col: Option<u16>,
    /// 「↓ 回到底部」按钮点击热区
    pub bottom_button: Rect,
    /// 底栏右侧上下文/用量点击热区
    pub usage_cell: Rect,
    /// 当前输入框外框区域（用于布局测量与交互定位）
    pub input_area: Rect,
    /// 当前输入框纯文字区（不含圆角边框、`❯` 提示符与迷你滚动条）
    pub input_text_area: Rect,
    /// 当前输入框文字区净列宽（供多行上下键光标移动与折行严格对齐）
    pub input_text_width: u16,
    pub sidebar: SidebarHits,
    pub body_rows: Vec<BodyRowHit>,
    pub fold_rows: HashMap<u16, (u64, FoldSection)>,
    pub total_body_rows: usize,
    pub visible_body_rows: usize,
    pub entry_starts: HashMap<u64, usize>,
    /// 正文各屏幕行的复制元信息（跳过代码块语言头/左侧装饰列，自动拼接软折行）
    pub row_copy_meta: HashMap<u16, RowCopyMeta>,
    /// 本帧扁平屏幕纯文本快照（整屏仅 3 次连续分配），用于所见即所得划词复制
    pub screen_snapshot: ScreenSnapshot,
}

impl FrameHits {
    /// 从当前帧快照中提取选区文字（跳过装饰列/语言标签行/右侧滚动条与边栏竖线、自动拼接软折行、不跨栏、去掉行尾补齐空格）。
    pub fn extract_selection(&self, sel: TextSelection) -> String {
        selection::extract_selection(
            &self.screen_snapshot,
            sel,
            self.body_text,
            self.scrollbar_col,
            self.sidebar_divider_col,
            &self.row_copy_meta,
        )
    }
}

pub struct ViewState<'a> {
    pub transcript: &'a Transcript,
    pub editor: &'a InputEditor,
    pub cwd: &'a str,
    pub agent_banner: &'a str,
    pub model_label: &'a str,
    pub effort_label: &'a str,
    pub mode_label: &'a str,
    pub tier: Option<&'a str>,
    pub busy: bool,
    pub connecting: bool,
    pub scroll: usize,
    pub auto_follow: bool,
    pub show_sidebar: bool,
    pub tick: usize,
    pub started_at: Instant,
    pub toast: Option<(&'a str, bool)>,
    pub background_tasks: &'a [BackgroundTask],
    pub subagents: &'a [crate::subagent::Agent],
    pub recent_sessions: &'a [SessionSummary],
    pub completion_menu: Option<&'a CompletionMenu>,
    pub completion_selected: usize,
    pub pending_images: usize,
    pub queued_mode: QueuedMode,
    pub queued_count: usize,
    pub queued_head_preview: Option<&'a str>,
    pub selection: Option<TextSelection>,
    pub hover: &'a HoverTarget,
    pub picker: Option<&'a PickerState>,
    pub permission: Option<(&'a PermissionRequest, usize)>,
    pub elicitation: Option<&'a ElicitationState>,
}

pub fn draw(frame: &mut Frame, view: &ViewState<'_>, cache: &mut MdCache) -> FrameHits {
    let full = frame.area();
    let p = theme::palette();

    // 全局背景铺底
    if p.background != ratatui::style::Color::Reset {
        frame.render_widget(
            Block::default().style(Style::new().bg(p.background).fg(p.text)),
            full,
        );
    }

    let is_home = view.transcript.entries.is_empty() && !view.busy;
    if is_home {
        return draw_home(frame, full, view);
    }

    // 宽屏双栏拆分（>= 115 列且侧边栏开启）
    let side_w: u16 = if view.show_sidebar && full.width >= 115 {
        36
    } else {
        0
    };
    let main_area = Rect::new(full.x, full.y, full.width.saturating_sub(side_w), full.height);
    let side_area = Rect::new(full.right().saturating_sub(side_w), full.y, side_w, full.height);

    // 主栏左右优雅留白
    let content_w = if main_area.width >= 86 {
        ((u32::from(main_area.width) * 92) / 100) as u16
    } else {
        main_area.width.saturating_sub(2).max(16)
    };
    let content_x = main_area.x + (main_area.width.saturating_sub(content_w)) / 2;

    // 底部区域：footer (1) + input/drawer + pulse (1) + gap
    let footer_h = 1u16;
    let footer_y = main_area.bottom().saturating_sub(footer_h);

    let (box_area, pulse_y, input_text_w) = if let Some((req, _)) = view.permission {
        let drawer_h = (req.options.len() as u16 + 5)
            .clamp(7, main_area.height / 2)
            .min(main_area.height.saturating_sub(3));
        let y = footer_y.saturating_sub(drawer_h);
        (
            Rect::new(content_x, y, content_w, drawer_h),
            y.saturating_sub(1),
            input_text_width(content_w),
        )
    } else if let Some(el) = view.elicitation {
        let drawer_h = elicitation::drawer_height(el, main_area.height);
        let y = footer_y.saturating_sub(drawer_h);
        (
            Rect::new(content_x, y, content_w, drawer_h),
            y.saturating_sub(1),
            input_text_width(content_w),
        )
    } else {
        let text_w = input_text_width(content_w);
        let vlines = view.editor.visual_lines(text_w);
        let max_inner = main_area
            .height
            .saturating_sub(9)
            .clamp(1, CHAT_INPUT_MAX_ROWS);
        let inner_rows = (vlines.len() as u16).clamp(1, max_inner);
        let box_h = inner_rows + 2;
        let y = footer_y.saturating_sub(box_h);
        (
            Rect::new(content_x, y, content_w, box_h),
            y.saturating_sub(1),
            text_w,
        )
    };

    let body_top = main_area.y + 1;
    // 正文底部与状态行之间留 1 行呼吸空白，状态行紧贴输入框/抽屉上方
    let body_bottom = pulse_y.saturating_sub(1).max(body_top);
    let body_area = Rect::new(
        content_x,
        body_top,
        content_w,
        body_bottom.saturating_sub(body_top),
    );

    // 1. 构建并渲染正文时间线
    let text_room = body_area.width.saturating_sub(2).max(16);
    let built = build_body(view.transcript, view.subagents, text_room, view.tick, cache);
    let total_rows = built.rows.len();
    let visible_rows = usize::from(body_area.height);
    let max_scroll = total_rows.saturating_sub(visible_rows);
    let scroll = if view.auto_follow {
        max_scroll
    } else {
        view.scroll.min(max_scroll)
    };

    let mut body_hits = Vec::new();
    let mut fold_rows = HashMap::new();
    let mut row_copy_meta = HashMap::new();
    let mut render_lines = Vec::with_capacity(visible_rows);
    for (offset, row) in built
        .rows
        .iter()
        .skip(scroll)
        .take(visible_rows)
        .enumerate()
    {
        let screen_y = body_area.y + offset as u16;
        body_hits.push((
            screen_y,
            row.entry_id,
            row.action == rows::RowAction::Toggle,
            row.links.clone(),
        ));
        row_copy_meta.insert(screen_y, row.copy_meta);
        if let rows::RowAction::Fold(section) = row.action {
            fold_rows.insert(screen_y, (row.entry_id, section));
            if *view.hover == HoverTarget::Fold(row.entry_id, section) {
                render_lines.push(Line::styled("点击展开10行，右击全部展开", theme::accent_bold().bg(p.row_focus_bg)));
                continue;
            }
        }
        render_lines.push(row_to_line(row));
    }
    frame.render_widget(
        Paragraph::new(render_lines),
        Rect::new(body_area.x, body_area.y, text_room, body_area.height),
    );

    // 细腻滚动条（命中热区向左扩 1 列，点击/拖拽更顺手）
    let mut scrollbar_hit = Rect::default();
    let mut scrollbar_col = None;
    if total_rows > visible_rows && body_area.height >= 3 {
        let bar_x = body_area.right().saturating_sub(1);
        scrollbar_col = Some(bar_x);
        draw_scrollbar(
            frame,
            Rect::new(bar_x, body_area.y, 1, body_area.height),
            total_rows,
            visible_rows,
            scroll,
        );
        let hit_x = bar_x.saturating_sub(1).max(body_area.x);
        scrollbar_hit = Rect::new(
            hit_x,
            body_area.y,
            body_area.right().saturating_sub(hit_x),
            body_area.height,
        );
    }

    // 计算当前视口对应的用户消息（供侧边栏 `❱` 指示）
    let current_user_entry = find_current_user_entry(view.transcript, &built.rows, scroll, view.auto_follow);

    // 2. 流光运行状态行 / Toast / 可点击的「回到底部」按钮（与输入框内沿、底栏左右对齐）
    let mut bottom_button = Rect::default();
    if pulse_y >= body_top && pulse_y < box_area.y {
        let pulse_area = Rect::new(
            content_x + 1,
            pulse_y,
            content_w.saturating_sub(2),
            1,
        );
        bottom_button = draw_pulse_line(
            frame,
            pulse_area,
            view,
            !view.auto_follow && scroll < max_scroll,
        );
    }

    // 3. 权限审批抽屉 / 代理提问抽屉 / 圆角输入框
    let input_text_area = if let Some((req, sel)) = view.permission {
        overlay::draw_permission_drawer(frame, box_area, req, sel);
        Rect::default()
    } else if let Some(el) = view.elicitation {
        elicitation::draw_elicitation_drawer(frame, box_area, el);
        Rect::default()
    } else {
        draw_input_box(
            frame,
            box_area,
            view.editor,
            view.picker.is_none(),
            false,
            InputBoxStatus {
                pending_images: view.pending_images,
                queued_mode: view.queued_mode,
                queued_count: view.queued_count,
                queued_head_preview: view.queued_head_preview,
                busy: view.busy,
            },
        )
    };

    // 4. 极简底栏（右下角展示用量与当前工作目录）
    let usage_cell = draw_footer(
        frame,
        Rect::new(content_x + 1, footer_y, content_w.saturating_sub(2), 1),
        view,
        true,
    );

    // 5. 右侧边栏
    let sidebar_hits = if side_w > 0 {
        sidebar::draw(
            frame,
            side_area,
            &SidebarData {
                transcript: view.transcript,
                cwd: view.cwd,
                mode_label: view.mode_label,
                tier: view.tier,
                current_user_entry,
                background_tasks: view.background_tasks,
                subagents: view.subagents,
                started_at: view.started_at,
                recent_sessions: view.recent_sessions,
                hover: view.hover,
            },
        )
    } else {
        SidebarHits::default()
    };

    // 6. 统一输入补全悬浮菜单（/ 命令 · # Skills · @ 文件） & 模态选择器
    if view.permission.is_none()
        && view.elicitation.is_none()
        && view.picker.is_none()
        && let Some(menu) = view.completion_menu
    {
        overlay::draw_completion_menu(frame, box_area, menu, view.completion_selected);
    }
    if let Some(picker) = view.picker {
        overlay::draw_picker(frame, full, picker);
    }

    // 抓取扁平屏幕纯文本快照（整屏单块连续内存），再叠加选区反色高亮
    let body_text = Rect::new(
        body_area.x,
        body_area.y,
        text_room.min(body_area.width.saturating_sub(2).max(1)),
        body_area.height,
    );
    let screen_snapshot = ScreenSnapshot::capture(frame.buffer_mut(), full);
    // 仅对紧凑行内链接叠加悬停下划线与微底色（不作用于正文大块或折叠行）
    if let HoverTarget::BodyLink {
        row,
        col_start,
        col_end,
    } = *view.hover
        && view.selection.is_none()
        && view.picker.is_none()
        && view.permission.is_none()
        && view.elicitation.is_none()
        && row < full.bottom()
    {
        let buf = frame.buffer_mut();
        let hover_bg = p.row_focus_bg;
        for x in col_start..col_end.min(full.right()) {
            let cell = &mut buf[(x, row)];
            cell.set_style(
                cell.style()
                    .bg(hover_bg)
                    .add_modifier(Modifier::UNDERLINED),
            );
        }
    }
    if let Some(sel) = view.selection {
        apply_selection_highlight(frame.buffer_mut(), full, sel, body_text, &row_copy_meta);
    }

    FrameHits {
        main_area,
        body: body_area,
        body_text,
        scrollbar: scrollbar_hit,
        scrollbar_col,
        sidebar_divider_col: (side_w > 0).then_some(side_area.x),
        bottom_button,
        usage_cell,
        input_area: box_area,
        input_text_area,
        input_text_width: input_text_w,
        sidebar: sidebar_hits,
        body_rows: body_hits,
        fold_rows,
        total_body_rows: total_rows,
        visible_body_rows: visible_rows,
        entry_starts: built.entry_starts,
        row_copy_meta,
        screen_snapshot,
    }
}

/// 极简居中启动首页（复刻 LiteMiyu `tui-new.webp`）。
fn draw_home(frame: &mut Frame, full: Rect, view: &ViewState<'_>) -> FrameHits {
    let box_w = full.width.saturating_sub(6).clamp(28, 80);
    let text_w = input_text_width(box_w);
    let max_inner = full
        .height
        .saturating_sub(10)
        .clamp(1, HOME_INPUT_MAX_ROWS);
    let inner_rows = (view.editor.visual_lines(text_w).len() as u16).clamp(1, max_inner);
    let box_h = inner_rows + 2;

    let box_x = full.x + (full.width.saturating_sub(box_w)) / 2;
    let box_y = full.y + (full.height.saturating_sub(box_h + 6)) / 2;
    let box_area = Rect::new(box_x, box_y, box_w, box_h);

    // 输入框上方轻量标题/连接状态
    if box_y >= full.y + 2 {
        let banner_area = Rect::new(box_x + 1, box_y - 2, box_w.saturating_sub(2), 1);
        if view.connecting {
            let anim_frame = activity::frame_at(view.started_at.elapsed());
            frame.render_widget(
                Paragraph::new(activity::render_status_line(
                    "正在连接 ACP 服务…",
                    "",
                    anim_frame,
                )),
                banner_area,
            );
        } else {
            let left = Span::styled("litemiyu-acp", theme::accent_bold());
            let sep = Span::styled(" · ", theme::faint());
            let right = Span::styled(view.agent_banner, theme::dim());
            frame.render_widget(Paragraph::new(Line::from(vec![left, sep, right])), banner_area);
        }
    }

    let input_text_area = if let Some((req, sel)) = view.permission {
        let dh = (req.options.len() as u16 + 5).clamp(7, 14);
        overlay::draw_permission_drawer(frame, Rect::new(box_x, box_y, box_w, dh), req, sel);
        Rect::default()
    } else if let Some(el) = view.elicitation {
        let dh = elicitation::drawer_height(el, full.height);
        elicitation::draw_elicitation_drawer(frame, Rect::new(box_x, box_y, box_w, dh), el);
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

    // 框下模型与工作区
    let info_y = box_area.bottom();
    let mut usage_cell = Rect::default();
    if info_y < full.bottom() {
        usage_cell = draw_footer(
            frame,
            Rect::new(box_x + 1, info_y, box_w.saturating_sub(2), 1),
            view,
            false,
        );
    }
    if info_y + 2 < full.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                linebreak::clip(view.cwd, box_w.saturating_sub(2)),
                theme::faint(),
            )),
            Rect::new(box_x + 1, info_y + 2, box_w.saturating_sub(2), 1),
        );
    }
    if info_y + 4 < full.bottom() {
        let hint = if let Some((msg, is_err)) = view.toast {
            Line::styled(msg, if is_err { theme::error() } else { theme::good() })
        } else {
            Line::styled(
                "/resume 恢复会话 · /model 切模型 · /theme 切主题 · Shift+Tab 切模式 · /help 帮助",
                theme::faint(),
            )
        };
        frame.render_widget(
            Paragraph::new(hint),
            Rect::new(box_x + 1, info_y + 4, box_w.saturating_sub(2), 1),
        );
    }

    if view.permission.is_none()
        && view.elicitation.is_none()
        && view.picker.is_none()
        && let Some(menu) = view.completion_menu
    {
        overlay::draw_completion_menu(frame, box_area, menu, view.completion_selected);
    }
    if let Some(picker) = view.picker {
        overlay::draw_picker(frame, full, picker);
    }

    let screen_snapshot = ScreenSnapshot::capture(frame.buffer_mut(), full);
    let empty_meta = HashMap::new();
    if let Some(sel) = view.selection {
        apply_selection_highlight(frame.buffer_mut(), full, sel, Rect::default(), &empty_meta);
    }

    FrameHits {
        main_area: full,
        usage_cell,
        input_area: box_area,
        input_text_area,
        input_text_width: text_w,
        screen_snapshot,
        ..FrameHits::default()
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, view: &ViewState<'_>, show_cwd: bool) -> Rect {
    let mut left_spans = vec![Span::styled(view.model_label.to_string(), theme::model())];
    if !view.effort_label.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.effort_label.to_string(), theme::accent()));
    }
    if !view.mode_label.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.mode_label.to_string(), theme::dim()));
    }
    if view.queued_mode == QueuedMode::SendImmediately {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(view.queued_mode.short_label(), theme::accent()));
    }
    if view.queued_count > 0 {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(
            format!("队列 {}", view.queued_count),
            theme::accent(),
        ));
    }
    if !view.background_tasks.is_empty() {
        left_spans.push(Span::styled(" · ", theme::faint()));
        left_spans.push(Span::styled(
            format!("后台 {}", view.background_tasks.len()),
            theme::accent(),
        ));
    }
    let left_w: usize = left_spans.iter().map(|s| s.content.width()).sum();
    frame.render_widget(Paragraph::new(Line::from(left_spans)), area);

    // 右侧：上下文与累计用量（可点击弹出状态/压缩面板）+ 当前工作目录
    let mut usage_parts = Vec::new();
    if view.transcript.context_window > 0 && view.transcript.context_used > 0 {
        let pct = (view.transcript.context_used as f64 / view.transcript.context_window as f64) * 100.0;
        usage_parts.push(format!(
            "{}/{}({:.1}%)",
            format_short(view.transcript.context_used),
            format_short(view.transcript.context_window),
            pct
        ));
    }
    let total = view.transcript.total_input
        + view.transcript.total_output
        + view.transcript.total_cached_read;
    if total > 0 {
        let mut sum = format!("Σ{}", format_short(total));
        let denom = view.transcript.total_input + view.transcript.total_cached_read;
        if denom > 0 && view.transcript.total_cached_read > 0 {
            let pct = (view.transcript.total_cached_read * 100) / denom;
            sum.push_str(&format!("(C{pct}%)"));
        }
        usage_parts.push(sum);
    }
    let usage_text = usage_parts.join(" · ");
    let usage_w = usage_text.width();
    let avail = usize::from(area.width).saturating_sub(left_w + 2);
    if avail == 0 {
        return Rect::default();
    }

    let mut right_spans: Vec<Span<'static>> = Vec::new();
    let mut show_usage = false;
    let mut cwd_w_used = 0usize;

    let cwd_budget = if show_cwd && !view.cwd.trim().is_empty() {
        if usage_w > 0 && avail >= usage_w + 11 {
            (avail - usage_w - 3).min(36)
        } else if usage_w == 0 && avail >= 6 {
            avail.min(44)
        } else if avail >= 10 && usage_w > avail {
            avail.min(36)
        } else {
            0
        }
    } else {
        0
    };

    if usage_w > 0 && usage_w <= avail {
        let usage_style = if matches!(view.hover, HoverTarget::UsageCell) {
            theme::accent()
                .bg(theme::palette().row_focus_bg)
                .add_modifier(Modifier::UNDERLINED)
        } else {
            theme::dim()
        };
        right_spans.push(Span::styled(usage_text.clone(), usage_style));
        show_usage = true;
    }

    if cwd_budget >= 6 {
        let short_cwd = overlay::shorten_path_left(view.cwd, cwd_budget as u16);
        if !short_cwd.is_empty() {
            if show_usage {
                right_spans.push(Span::styled(" · ", theme::faint()));
                cwd_w_used = 3 + short_cwd.width();
            } else {
                cwd_w_used = short_cwd.width();
            }
            right_spans.push(Span::styled(short_cwd, theme::faint()));
        }
    }

    if !right_spans.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(right_spans).right_aligned()),
            area,
        );
    }

    if show_usage {
        let w = u16::try_from(usage_w).unwrap_or(0).min(area.width);
        let right_offset = u16::try_from(cwd_w_used).unwrap_or(0);
        let x = area
            .right()
            .saturating_sub(right_offset)
            .saturating_sub(w)
            .max(area.x);
        return Rect::new(x, area.y, w, 1);
    }
    Rect::default()
}

fn find_current_user_entry(
    transcript: &Transcript,
    rows: &[RenderRow],
    scroll: usize,
    auto_follow: bool,
) -> Option<u64> {
    if auto_follow {
        return transcript
            .entries
            .iter()
            .rev()
            .find(|e| matches!(e.kind, crate::transcript::EntryKind::User { .. }))
            .map(|e| e.id);
    }
    let top_id = rows.get(scroll).map(|r| r.entry_id)?;
    let pos = transcript.entries.iter().position(|e| e.id == top_id)?;
    transcript.entries[..=pos]
        .iter()
        .rev()
        .find(|e| matches!(e.kind, crate::transcript::EntryKind::User { .. }))
        .map(|e| e.id)
}
