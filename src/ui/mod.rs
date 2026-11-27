//! 主 TUI 渲染协调器（复刻 LiteMiyu 首页 + 对话双栏视图 + 流光状态行 + 圆角输入框 + 极简底栏）。

pub mod activity;
mod elicitation;
mod footer;
pub mod format;
pub mod home;
mod input_box;
#[cfg(test)]
mod fold_tests;
#[cfg(test)]
mod interaction_tests;
pub mod overlay;
mod overlay_drawers;
pub mod overlay_hits;
pub mod rows;
pub mod selection;
pub mod sidebar;
mod sticky_prompt;
#[cfg(test)]
mod tests;
mod todo_bar;

use std::collections::HashMap;
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Paragraph};

use crate::acp::{PermissionRequest, SessionSummary};
use crate::app::{ElicitationState, QueuedMode};
use crate::background::BackgroundTask;
use crate::completion::CompletionMenu;
use crate::input::InputEditor;
use crate::theme;
use crate::transcript::{FoldSection, Transcript};

pub use home::{HomeAction, HomeHits};
pub use overlay_hits::{OverlayFooterAction, OverlayHits};

/// 对话界面（非开始界面）输入框最大可见行数（内容增加时自动增高，超过后内部滚动）。
pub const CHAT_INPUT_MAX_ROWS: u16 = 8;
/// 居中启动首页输入框最大可见行数。
pub const HOME_INPUT_MAX_ROWS: u16 = 6;

/// 统一计算圆角输入框内文字区的净列宽（扣除左右边框 2 + 左侧 ` ❯ ` 3 + 右侧留白/滚动条 1 = 6）。
pub fn input_text_width(box_w: u16) -> u16 {
    box_w.saturating_sub(6).max(1)
}

use overlay::PickerState;
use rows::{InlineAction, InlineButton, MdCache, RenderRow, RowCopyMeta, build_body, row_to_line};
use input_box::{InputBoxStatus, draw_input_box, draw_pulse_line, draw_scrollbar};
use selection::{ScreenSnapshot, apply_selection_highlight};
pub use selection::TextSelection;
use sidebar::{SidebarData, SidebarHits};
use sticky_prompt::StickyPromptHit;

/// 正文单行命中热区元信息（屏幕行、条目 ID、是否可折叠、行内链接列表）。
pub type BodyRowHit = (u16, u64, bool, Vec<(u16, u16, String)>);

/// 紧凑可交互组件的鼠标悬停目标（不含正文大块与折叠区，避免大面积闪烁）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HoverTarget {
    #[default]
    None,
    SidebarBackground(u64),
    SidebarSubagent(u64),
    SidebarTodo,
    InlineButton {
        row: u16,
        col_start: u16,
        col_end: u16,
        entry_id: u64,
        action: InlineAction,
    },
    SidebarJump(u64),
    SidebarSession(String),
    SidebarContext,
    TodoBarToggle,
    StickyUserPrompt(u64),
    HomeAction(usize, HomeAction),
    HomeSession(String),
    OverlayAction(OverlayFooterAction),
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
    /// 底部停靠任务条点击展开/收起热区
    pub todo_bar_toggle: Rect,
    /// 主视图顶部黏性上条用户消息点击跳转热区
    pub sticky_prompt: StickyPromptHit,
    /// 启动首页交互热区（模型/快捷操作/最近会话）
    pub home: HomeHits,
    /// 活跃浮层、权限/提问抽屉与补全菜单的鼠标命中热区
    pub overlay: OverlayHits,
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
    pub inline_buttons: HashMap<u16, (u64, Vec<InlineButton>)>,
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
    pub git_branch: Option<&'a str>,
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
    pub todo_bar_expanded: bool,
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
        return home::draw(frame, full, view);
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
    let todo_bar_h = todo_bar::measure_height(
        &view.transcript.latest_plan,
        view.todo_bar_expanded,
        main_area.height,
    );
    let todo_area = if todo_bar_h > 0 && pulse_y >= body_top.saturating_add(todo_bar_h + 1) {
        Rect::new(
            content_x + 1,
            pulse_y.saturating_sub(todo_bar_h),
            content_w.saturating_sub(2),
            todo_bar_h,
        )
    } else {
        Rect::default()
    };
    // 正文底部与停靠任务栏/状态行之间留 1 行呼吸空白，状态行紧贴输入框/抽屉上方
    let chrome_top = if todo_area.height > 0 {
        todo_area.y
    } else {
        pulse_y
    };
    let body_bottom = chrome_top.saturating_sub(1).max(body_top);
    let text_room = content_w.saturating_sub(2).max(16);

    // 1. 构建正文时间线，并根据当前滚动位置决定是否在主视图顶部黏住上条用户消息
    let built = build_body(
        view.transcript,
        view.subagents,
        text_room,
        view.busy,
        view.tick,
        cache,
    );
    let total_rows = built.rows.len();
    let base_visible = usize::from(body_bottom.saturating_sub(body_top));
    let sticky_body_top = main_area.y.saturating_add(3);
    let sticky_visible = usize::from(body_bottom.saturating_sub(sticky_body_top));
    let sticky_max_scroll = total_rows.saturating_sub(sticky_visible);
    // 仅当内容超过一屏可滚动且高度足够时启用顶部黏性条，并用实际滚动偏移判定避免底部临界闪烁
    let sticky_data = if body_bottom >= main_area.y.saturating_add(6) && total_rows > base_visible {
        let probe_scroll = if view.auto_follow {
            sticky_max_scroll
        } else {
            view.scroll.min(sticky_max_scroll)
        };
        sticky_prompt::resolve(view.transcript, &built.entry_starts, probe_scroll)
    } else {
        None
    };

    let (actual_body_top, sticky_hit) = if let Some((eid, raw_text)) = sticky_data {
        // 顶部保留 1 行呼吸上边距（与右侧边栏顶沿齐平），黏性条下方再留 1 行空白分隔正文
        let sticky_area = Rect::new(content_x, main_area.y.saturating_add(1), text_room, 1);
        let hit = sticky_prompt::draw(frame, sticky_area, eid, raw_text, view.hover);
        (sticky_body_top, hit)
    } else {
        (body_top, StickyPromptHit::default())
    };
    let body_area = Rect::new(
        content_x,
        actual_body_top,
        content_w,
        body_bottom.saturating_sub(actual_body_top),
    );
    let visible_rows = usize::from(body_area.height);
    let max_scroll = total_rows.saturating_sub(visible_rows);
    let scroll = if view.auto_follow {
        max_scroll
    } else {
        view.scroll.min(max_scroll)
    };

    let mut body_hits = Vec::new();
    let mut fold_rows = HashMap::new();
    let mut inline_buttons = HashMap::new();
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
        }
        if !row.buttons.is_empty() {
            inline_buttons.insert(screen_y, (row.entry_id, row.buttons.clone()));
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

    // 1.5 底部停靠任务清单条（窄屏/隐藏侧栏/手动展开时展示，点击或按 Ctrl+T 切换展开）
    let todo_bar_toggle = if todo_area.height > 0 {
        todo_bar::draw(
            frame,
            todo_area,
            &view.transcript.latest_plan,
            view.todo_bar_expanded,
            view.busy,
            view.tick,
            view.hover,
        )
    } else {
        Rect::default()
    };

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
    let mut overlay_hits = OverlayHits::default();
    let input_text_area = if let Some((req, sel)) = view.permission {
        overlay_hits.permission =
            overlay::draw_permission_drawer(frame, box_area, req, sel, view.hover);
        Rect::default()
    } else if let Some(el) = view.elicitation {
        overlay_hits.elicitation =
            elicitation::draw_elicitation_drawer(frame, box_area, el, view.hover);
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

    // 4. 极简底栏（右下角分层着色展示用量、工作目录与 Git 分支）
    let usage_cell = footer::draw(
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
                busy: view.busy,
                tick: view.tick,
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

    // 抓取扁平屏幕纯文本快照（整屏单块连续内存），再叠加选区反色高亮
    let body_text = Rect::new(
        body_area.x,
        body_area.y,
        text_room.min(body_area.width.saturating_sub(2).max(1)),
        body_area.height,
    );
    let screen_snapshot = ScreenSnapshot::capture(frame.buffer_mut(), full);
    // 折叠展开/完整文件/收起按钮：悬停仅高亮前景文字，绝不铺设背景色
    if let HoverTarget::InlineButton {
        row,
        col_start,
        col_end,
        ..
    } = *view.hover
        && view.selection.is_none()
        && view.picker.is_none()
        && view.permission.is_none()
        && view.elicitation.is_none()
        && row < full.bottom()
    {
        let buf = frame.buffer_mut();
        let hi_fg = theme::accent_bold().fg.unwrap_or(p.accent);
        for x in col_start..col_end.min(full.right()) {
            let cell = &mut buf[(x, row)];
            cell.set_style(cell.style().fg(hi_fg).add_modifier(Modifier::BOLD));
        }
    }
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
        todo_bar_toggle,
        sticky_prompt: sticky_hit,
        home: HomeHits::default(),
        overlay: overlay_hits,
        usage_cell,
        input_area: box_area,
        input_text_area,
        input_text_width: input_text_w,
        sidebar: sidebar_hits,
        body_rows: body_hits,
        fold_rows,
        inline_buttons,
        total_body_rows: total_rows,
        visible_body_rows: visible_rows,
        entry_starts: built.entry_starts,
        row_copy_meta,
        screen_snapshot,
    }
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
