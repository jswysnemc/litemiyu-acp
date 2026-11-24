//! 主 TUI 渲染协调器（复刻 LiteMiyu 首页 + 对话双栏视图 + 流光状态行 + 圆角输入框 + 极简底栏）。

pub mod activity;
pub mod overlay;
pub mod rows;
pub mod sidebar;

use std::collections::HashMap;
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::acp::{PermissionRequest, SessionSummary};
use crate::input::{AtomKind, AtomSpan, InputEditor};
use crate::linebreak;
use crate::theme;
use crate::transcript::{Transcript, format_short};

/// 对话界面（非开始界面）输入框最大可见行数（内容增加时自动增高，超过后内部滚动）。
pub const CHAT_INPUT_MAX_ROWS: u16 = 8;
/// 居中启动首页输入框最大可见行数。
pub const HOME_INPUT_MAX_ROWS: u16 = 6;

/// 统一计算圆角输入框内文字区的净列宽（扣除左右边框 2 + 左侧 ` ❯ ` 3 + 右侧留白/滚动条 1 = 6）。
pub fn input_text_width(box_w: u16) -> u16 {
    box_w.saturating_sub(6).max(1)
}

use overlay::{PickerState, SlashSuggestion};
use rows::{MdCache, RenderRow, RowCopyMeta, build_body, row_to_line};
use sidebar::{SidebarData, SidebarHits};

#[derive(Debug, Clone, Copy)]
pub struct TextSelection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// 限制选区的列边界（避免在正文区拖选时误框入右侧边栏或滚动条）
    pub zone: Rect,
}

impl TextSelection {
    /// 返回规范化后的 `(start, end)`，按 `(row, col)` 从小到大排序。
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let a = (self.anchor.1, self.anchor.0);
        let b = (self.head.1, self.head.0);
        if a <= b {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn row_col_range(&self, y: u16) -> Option<(u16, u16)> {
        let ((sx, sy), (ex, ey)) = self.ordered();
        if y < sy || y > ey || self.zone.width == 0 {
            return None;
        }
        let z_left = self.zone.x;
        let z_right = self.zone.right().saturating_sub(1);
        let from = if y == sy { sx.clamp(z_left, z_right) } else { z_left };
        let to = if y == ey { ex.clamp(z_left, z_right) } else { z_right };
        (from <= to).then_some((from, to))
    }
}

#[derive(Default, Debug, Clone)]
pub struct FrameHits {
    pub body: Rect,
    /// 正文区纯文字区域（不含右侧滚动条列）
    pub body_text: Rect,
    /// 正文右侧滚动条点击/拖动热区
    pub scrollbar: Rect,
    /// 「↓ 回到底部」按钮点击热区
    pub bottom_button: Rect,
    /// 底栏右侧上下文/用量点击热区
    pub usage_cell: Rect,
    /// 当前输入框外框区域（用于布局测量与交互定位）
    #[allow(dead_code)]
    pub input_area: Rect,
    /// 当前输入框文字区净列宽（供多行上下键光标移动与折行严格对齐）
    pub input_text_width: u16,
    pub sidebar: SidebarHits,
    pub body_rows: Vec<(u16, u64, bool, Vec<(u16, u16, String)>)>,
    pub total_body_rows: usize,
    pub visible_body_rows: usize,
    pub entry_starts: HashMap<u64, usize>,
    /// 正文各屏幕行的复制元信息（跳过代码块语言头/左侧装饰列，自动拼接软折行）
    pub row_copy_meta: HashMap<u16, RowCopyMeta>,
    /// 本帧完整屏幕网格（按行存 `(col, symbol)`），用于 100% 所见即所得精准划词复制
    pub screen_lines: Vec<Vec<(u16, String)>>,
}

impl FrameHits {
    /// 从当前帧快照中提取选区文字（跳过装饰列/语言标签行、自动拼接软折行、不跨栏、去掉行尾补齐空格）。
    pub fn extract_selection(&self, sel: TextSelection) -> String {
        let ((_, sy), (_, ey)) = sel.ordered();
        let multi_row = sy < ey;
        let in_body = self.body_text.width > 0 && sel.zone == self.body_text;
        let mut lines: Vec<String> = Vec::new();
        let mut prev_emitted_y: Option<u16> = None;

        for y in sy..=ey {
            let Some((raw_from, to)) = sel.row_col_range(y) else {
                continue;
            };
            let meta = if in_body {
                self.row_copy_meta.get(&y).copied()
            } else {
                None
            };
            // 多行框选时自动跳过代码块语言标签、空竖线等纯装饰行
            if multi_row && meta.is_some_and(|m| m.decorative) {
                continue;
            }
            let from = if let Some(m) = meta {
                raw_from.max(self.body_text.x.saturating_add(m.content_left))
            } else {
                raw_from
            };
            if from > to {
                continue;
            }
            let Some(cells) = self.screen_lines.get(usize::from(y)) else {
                continue;
            };
            let mut line = String::new();
            let mut skip_until = 0u16;
            for (x, sym) in cells {
                if *x < skip_until {
                    continue;
                }
                let w = u16::try_from(sym.width()).unwrap_or(1).max(1);
                if *x + w > from && *x <= to {
                    line.push_str(sym);
                }
                skip_until = *x + w;
            }
            let trimmed = line.trim_end().to_string();
            if meta.is_some_and(|m| m.join_previous)
                && prev_emitted_y == Some(y.saturating_sub(1))
                && !lines.is_empty()
            {
                if let Some(last) = lines.last_mut() {
                    last.push_str(&trimmed);
                }
            } else {
                lines.push(trimmed);
            }
            prev_emitted_y = Some(y);
        }
        // 去掉首尾全空行（保留中间有意义的空行）
        while lines.first().is_some_and(|l| l.is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
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
    pub recent_sessions: &'a [SessionSummary],
    pub slash_menu: &'a [SlashSuggestion],
    pub slash_selected: usize,
    pub pending_images: usize,
    pub selection: Option<TextSelection>,
    pub picker: Option<&'a PickerState>,
    pub permission: Option<(&'a PermissionRequest, usize)>,
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
    } else {
        let text_w = input_text_width(content_w);
        let vlines = view.editor.visual_lines(text_w);
        let max_inner = main_area
            .height
            .saturating_sub(9)
            .min(CHAT_INPUT_MAX_ROWS)
            .max(1);
        let inner_rows = (vlines.len() as u16).clamp(1, max_inner);
        let box_h = inner_rows + 2;
        let y = footer_y.saturating_sub(box_h);
        (
            Rect::new(content_x, y, content_w, box_h),
            y.saturating_sub(2),
            text_w,
        )
    };

    let body_top = main_area.y + 1;
    let body_bottom = pulse_y.saturating_sub(0).max(body_top);
    let body_area = Rect::new(
        content_x,
        body_top,
        content_w,
        body_bottom.saturating_sub(body_top),
    );

    // 1. 构建并渲染正文时间线
    let text_room = body_area.width.saturating_sub(2).max(16);
    let built = build_body(view.transcript, text_room, view.tick, cache);
    let total_rows = built.rows.len();
    let visible_rows = usize::from(body_area.height);
    let max_scroll = total_rows.saturating_sub(visible_rows);
    let scroll = if view.auto_follow {
        max_scroll
    } else {
        view.scroll.min(max_scroll)
    };

    let mut body_hits = Vec::new();
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
            row.clickable_entry,
            row.links.clone(),
        ));
        row_copy_meta.insert(screen_y, row.copy_meta);
        render_lines.push(row_to_line(row));
    }
    frame.render_widget(
        Paragraph::new(render_lines),
        Rect::new(body_area.x, body_area.y, text_room, body_area.height),
    );

    // 细腻滚动条（命中热区向左扩 1 列，点击/拖拽更顺手）
    let mut scrollbar_hit = Rect::default();
    if total_rows > visible_rows && body_area.height >= 3 {
        let bar_x = body_area.right().saturating_sub(1);
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

    // 2. 流光运行状态行 / Toast / 可点击的「回到底部」按钮
    let mut bottom_button = Rect::default();
    if pulse_y >= body_top && pulse_y < box_area.y {
        let pulse_area = Rect::new(content_x, pulse_y, content_w, 1);
        bottom_button = draw_pulse_line(
            frame,
            pulse_area,
            view,
            !view.auto_follow && scroll < max_scroll,
        );
    }

    // 3. 权限审批抽屉 或 圆角输入框
    if let Some((req, sel)) = view.permission {
        overlay::draw_permission_drawer(frame, box_area, req, sel);
    } else {
        draw_input_box(
            frame,
            box_area,
            view.editor,
            view.picker.is_none(),
            false,
            view.pending_images,
        );
    }

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
                recent_sessions: view.recent_sessions,
            },
        )
    } else {
        SidebarHits::default()
    };

    // 6. 斜杠命令悬浮菜单 & 模态选择器
    if view.permission.is_none() && view.picker.is_none() && !view.slash_menu.is_empty() {
        overlay::draw_slash_menu(frame, box_area, view.slash_menu, view.slash_selected);
    }
    if let Some(picker) = view.picker {
        overlay::draw_picker(frame, full, picker);
    }

    // 先抓一份本帧纯文本网格，再给选区叠加反色高亮（确保选区与屏幕文字 100% 一致）
    let body_text = Rect::new(body_area.x, body_area.y, text_room, body_area.height);
    let screen_lines = snapshot_buffer(frame.buffer_mut(), full);
    if let Some(sel) = view.selection {
        apply_selection_highlight(frame.buffer_mut(), full, sel, body_text, &row_copy_meta);
    }

    FrameHits {
        body: body_area,
        body_text,
        scrollbar: scrollbar_hit,
        bottom_button,
        usage_cell,
        input_area: box_area,
        input_text_width: input_text_w,
        sidebar: sidebar_hits,
        body_rows: body_hits,
        total_body_rows: total_rows,
        visible_body_rows: visible_rows,
        entry_starts: built.entry_starts,
        row_copy_meta,
        screen_lines,
    }
}

/// 极简居中启动首页（复刻 LiteMiyu `tui-new.webp`）。
fn draw_home(frame: &mut Frame, full: Rect, view: &ViewState<'_>) -> FrameHits {
    let box_w = full.width.saturating_sub(6).min(80).max(28);
    let text_w = input_text_width(box_w);
    let max_inner = full
        .height
        .saturating_sub(10)
        .min(HOME_INPUT_MAX_ROWS)
        .max(1);
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

    if let Some((req, sel)) = view.permission {
        let dh = (req.options.len() as u16 + 5).clamp(7, 14);
        overlay::draw_permission_drawer(frame, Rect::new(box_x, box_y, box_w, dh), req, sel);
    } else {
        draw_input_box(
            frame,
            box_area,
            view.editor,
            view.picker.is_none(),
            true,
            view.pending_images,
        );
    }

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

    if view.permission.is_none() && view.picker.is_none() && !view.slash_menu.is_empty() {
        overlay::draw_slash_menu(frame, box_area, view.slash_menu, view.slash_selected);
    }
    if let Some(picker) = view.picker {
        overlay::draw_picker(frame, full, picker);
    }

    let screen_lines = snapshot_buffer(frame.buffer_mut(), full);
    let empty_meta = HashMap::new();
    if let Some(sel) = view.selection {
        apply_selection_highlight(frame.buffer_mut(), full, sel, Rect::default(), &empty_meta);
    }

    FrameHits {
        usage_cell,
        input_area: box_area,
        input_text_width: text_w,
        screen_lines,
        ..FrameHits::default()
    }
}

fn snapshot_buffer(buf: &ratatui::buffer::Buffer, area: Rect) -> Vec<Vec<(u16, String)>> {
    let mut rows = Vec::with_capacity(usize::from(area.bottom()));
    for y in 0..area.bottom() {
        let mut line = Vec::with_capacity(usize::from(area.width));
        if y >= area.y {
            for x in area.x..area.right() {
                line.push((x, buf[(x, y)].symbol().to_string()));
            }
        }
        rows.push(line);
    }
    rows
}

fn apply_selection_highlight(
    buf: &mut ratatui::buffer::Buffer,
    full: Rect,
    sel: TextSelection,
    body_text: Rect,
    row_copy_meta: &HashMap<u16, RowCopyMeta>,
) {
    let ((_, sy), (_, ey)) = sel.ordered();
    let multi_row = sy < ey;
    let in_body = body_text.width > 0 && sel.zone == body_text;
    let max_y = full.bottom().saturating_sub(1);
    for y in sy.max(full.y)..=ey.min(max_y) {
        let meta = if in_body {
            row_copy_meta.get(&y).copied()
        } else {
            None
        };
        if multi_row && meta.is_some_and(|m| m.decorative) {
            continue;
        }
        if let Some((raw_from, to)) = sel.row_col_range(y) {
            let from = if let Some(m) = meta {
                raw_from.max(body_text.x.saturating_add(m.content_left))
            } else {
                raw_from
            };
            if from <= to {
                let rect =
                    Rect::new(from, y, to.saturating_sub(from) + 1, 1).intersection(full);
                buf.set_style(rect, Style::new().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn draw_input_box(
    frame: &mut Frame,
    area: Rect,
    editor: &InputEditor,
    show_cursor: bool,
    is_home: bool,
    pending_images: usize,
) {
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
        return;
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
            "/ 打开命令列表 · Ctrl+V/Alt+V 粘贴截图 · 拖拽图片直传"
        } else {
            "/ 命令 · Ctrl+V/Alt+V 贴图 · Shift+Enter 换行 · Ctrl+O 展开细节"
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
        return;
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

fn draw_pulse_line(
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
        let elapsed = view
            .transcript
            .turn_started
            .map(|s| activity::format_elapsed(s.elapsed()))
            .unwrap_or_default();
        let anim_frame = activity::frame_at(view.started_at.elapsed());
        let label = activity::infer_work_status(view.transcript);
        let status_line = activity::render_status_line(label, &elapsed, anim_frame);

        let (right_line, right_w, btn_hit) = if scrolled_up {
            let btn = "↓ 回到底部";
            let esc = "Esc 中断 · ";
            let rw = u16::try_from(esc.width() + btn.width())
                .unwrap_or(0)
                .min(area.width);
            let btn_w = u16::try_from(btn.width()).unwrap_or(0).min(area.width);
            let line = Line::from(vec![
                Span::styled(esc, theme::faint()),
                Span::styled(btn, theme::accent_bold()),
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
    if let Some((msg, is_err)) = view.toast {
        let style = if is_err { theme::error() } else { theme::good() };
        frame.render_widget(Paragraph::new(Line::styled(msg, style)), area);
    }
    if scrolled_up {
        let label = " ↓ 回到底部 (End) ";
        let hint = Line::styled(label, theme::accent_bold().bg(theme::palette().surface)).right_aligned();
        frame.render_widget(Paragraph::new(hint), area);
        let btn_w = u16::try_from(label.width()).unwrap_or(0).min(area.width);
        return Rect::new(area.right().saturating_sub(btn_w), area.y, btn_w, 1);
    }
    Rect::default()
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
        right_spans.push(Span::styled(usage_text.clone(), theme::dim()));
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

fn draw_scrollbar(frame: &mut Frame, area: Rect, total: usize, visible: usize, scroll: usize) {
    let h = usize::from(area.height);
    if h == 0 || total <= visible {
        return;
    }
    let thumb_h = ((visible * h) / total).clamp(1, h);
    let max_scroll = total - visible;
    let thumb_top = if max_scroll == 0 {
        0
    } else {
        (scroll * (h - thumb_h)) / max_scroll
    };
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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use crate::app::App;
    use crate::theme::{self, ThemePreset};

    fn buffer_to_text(buf: &ratatui::buffer::Buffer, w: u16, h: u16) -> String {
        let mut rendered = String::new();
        for y in 0..h {
            let mut x = 0;
            while x < w {
                let sym = buf[(x, y)].symbol();
                rendered.push_str(sym);
                x += u16::try_from(sym.width()).unwrap_or(1).max(1);
            }
            rendered.push('\n');
        }
        rendered
    }

    #[test]
    fn demo_screen_renders_cleanly_at_wide_and_narrow_sizes() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        app.auto_follow = false;
        app.scroll = 0;

        for (w, h) in [(140, 46), (84, 34)] {
            app.auto_follow = false;
            app.scroll = 0;
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let top_view = buffer_to_text(terminal.backend().buffer(), w, h);
            assert!(top_view.contains("修复方案输出"), "missing heading at {w}x{h}");
            assert!(top_view.contains("┌"), "missing boxed table at {w}x{h}");
            if w >= 115 {
                assert!(top_view.contains("工作区"), "missing sidebar at {w}x{h}");
                assert!(top_view.contains("▰"), "missing context bar fill at {w}x{h}");
                assert!(
                    top_view.contains("3分前") && top_view.contains("昨天"),
                    "sidebar recent sessions should show compact relative time at {w}x{h}:\n{top_view}"
                );
            }

            // 滚到底部验证新版 sai 风格代码块、右下角当前目录与 LiteMiyu 收尾元信息行
            app.auto_follow = true;
            terminal.draw(|f| app.render(f)).unwrap();
            let bottom_view = buffer_to_text(terminal.backend().buffer(), w, h);
            assert!(bottom_view.contains("kotlin"), "missing code language tag at {w}x{h}:\n{bottom_view}");
            assert!(
                !bottom_view.contains("╭─ code") && !bottom_view.contains('▎'),
                "code block should have no decorative borders/rails"
            );
            assert!(bottom_view.contains("▣"), "missing done meta row at {w}x{h}");
            let footer_row = bottom_view.lines().last().unwrap_or("");
            assert!(
                footer_row.contains("litemiyu-acp"),
                "bottom-right footer should show current working directory at {w}x{h}: {footer_row:?}"
            );

            // 验证框选整块代码块时自动忽略语言标签与左侧边距，只复制纯净代码（保留代码自身缩进）
            if w == 140 {
                let bz = app.last_hits.body_text;
                let code_header_y = app
                    .last_hits
                    .row_copy_meta
                    .iter()
                    .find(|(y, m)| {
                        m.decorative
                            && app
                                .last_hits
                                .screen_lines
                                .get(usize::from(**y))
                                .is_some_and(|cells| {
                                    cells
                                        .iter()
                                        .filter(|(x, _)| *x >= bz.x && *x < bz.right())
                                        .map(|(_, s)| s.as_str())
                                        .collect::<String>()
                                        .trim()
                                        == "kotlin"
                                })
                    })
                    .map(|(y, _)| *y)
                    .expect("find kotlin code header row");
                let code_sel = crate::ui::TextSelection {
                    anchor: (bz.x, code_header_y),
                    head: (bz.right() - 1, code_header_y + 7),
                    zone: bz,
                };
                let copied_code = app.last_hits.extract_selection(code_sel);
                assert!(
                    !copied_code.contains("kotlin\n") && !copied_code.starts_with("kotlin"),
                    "code copy must skip decorative language header:\n{copied_code}"
                );
                assert!(
                    copied_code.starts_with("// OffToLockscreenTransitionViewModel.kt"),
                    "code copy must strip decorative left margin without extra spaces:\n{copied_code:?}"
                );
                assert!(
                    copied_code.contains("\n    transitionAnimation.sharedFlow("),
                    "code copy must preserve code's own indentation:\n{copied_code:?}"
                );
            }

            // 验证正文区跨行划词提取：精准提取正文且绝不串入右侧边栏
            if w == 140 {
                let bz = app.last_hits.body_text;
                let sel = crate::ui::TextSelection {
                    anchor: (bz.x, bz.y),
                    head: (bz.right() - 1, bz.y + 8),
                    zone: bz,
                };
                let picked = app.last_hits.extract_selection(sel);
                assert!(!picked.is_empty(), "selection should not be empty");
                assert!(!picked.contains("工作区"), "body selection must not bleed into sidebar: {picked}");

                // 验证点击滚动条、回到底部按钮与上下文进度条
                app.auto_follow = false;
                app.scroll = 0;
                terminal.draw(|f| app.render(f)).unwrap();
                let bar = app.last_hits.scrollbar;
                let btn = app.last_hits.bottom_button;
                let ctx = app.last_hits.sidebar.context_area;
                assert!(bar.width > 0 && bar.height > 0, "scrollbar hitbox should exist");
                assert!(btn.width > 0, "bottom button hitbox should exist when scrolled up");
                assert!(ctx.width > 0 && ctx.height > 0, "context bar hitbox should exist");

                // 1) 点击滚动条中段
                let mid_y = bar.y + bar.height / 2;
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column: bar.x,
                    row: mid_y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                    column: bar.x,
                    row: mid_y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                assert!(app.scroll > 0, "clicking scrollbar midpoint should scroll down");

                // 2) 点击回到底部按钮
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column: btn.x + 1,
                    row: btn.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                    column: btn.x + 1,
                    row: btn.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                assert!(app.auto_follow, "clicking bottom button should restore auto_follow");

                // 3) 点击侧边栏上下文进度条弹出状态/压缩浮层
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column: ctx.x + 2,
                    row: ctx.bottom() - 1,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                    column: ctx.x + 2,
                    row: ctx.bottom() - 1,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                assert!(
                    app.picker
                        .as_ref()
                        .is_some_and(|p| p.kind == crate::ui::overlay::PickerKind::Status),
                    "clicking context progress bar should open status/compact picker"
                );
                app.picker = None;

                // 4) 验证读取与编辑工具均默认折叠（显示行号范围/增删统计），展开后呈现 sai 风格预览与 Diff
                app.auto_follow = false;
                app.scroll = 0;
                terminal.draw(|f| app.render(f)).unwrap();
                let collapsed_view = buffer_to_text(terminal.backend().buffer(), w, h);
                assert!(collapsed_view.contains("44–48"), "missing read range badge:\n{collapsed_view}");
                assert!(collapsed_view.contains("+1 -0"), "missing diff summary counts:\n{collapsed_view}");
                assert!(
                    !collapsed_view.contains("44  val lockscreenAlpha"),
                    "read preview should be collapsed by default:\n{collapsed_view}"
                );
                assert!(
                    !collapsed_view.contains("+          onFinish = { 1f },"),
                    "diff should be collapsed by default:\n{collapsed_view}"
                );

                // 点击读取行（按 diff 相同的点击打开逻辑）单独展开读取预览
                let read_screen_y = collapsed_view
                    .lines()
                    .enumerate()
                    .find(|(_, l)| l.contains("读取 · OffToLockscreenTransitionViewModel.kt"))
                    .map(|(y, _)| y as u16)
                    .expect("find read tool header row");
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column: bz.x + 2,
                    row: read_screen_y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                app.handle_mouse(crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                    column: bz.x + 2,
                    row: read_screen_y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                });
                terminal.draw(|f| app.render(f)).unwrap();
                let read_open_view = buffer_to_text(terminal.backend().buffer(), w, h);
                assert!(
                    read_open_view.contains("44  val lockscreenAlpha"),
                    "clicking read tool should open sai-style numbered code preview:\n{read_open_view}"
                );
                assert!(
                    read_open_view.contains("• D:\\Code\\Systemui-dev")
                        || read_open_view.contains("• D:/Code/Systemui-dev"),
                    "expanded read tool should show clickable file path like diff:\n{read_open_view}"
                );

                app.transcript.toggle_all_details(true);
                terminal.draw(|f| app.render(f)).unwrap();
                let diff_view = buffer_to_text(terminal.backend().buffer(), w, h);
                assert!(
                    diff_view.contains("+          onFinish = { 1f },"),
                    "missing rendered sai-style unified diff line after expand:\n{diff_view}"
                );
                app.transcript.toggle_all_details(false);
            }
        }
    }

    #[test]
    fn chat_input_box_grows_dynamically_and_caps_at_max_rows() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        // 1 行时框高 = 3 (1 + 2 边框)
        app.editor.set_text("hello");
        terminal.draw(|f| app.render(f)).unwrap();
        assert_eq!(app.last_hits.input_area.height, 3);

        // 4 行时自动增高到 6 (4 + 2 边框)
        app.editor.set_text("line1\nline2\nline3\nline4");
        terminal.draw(|f| app.render(f)).unwrap();
        assert_eq!(app.last_hits.input_area.height, 6);

        // 12 行时受 CHAT_INPUT_MAX_ROWS (8) 上限约束，框高 = 10，并显示行号提示
        let twelve = (1..=12).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
        app.editor.set_text(twelve);
        terminal.draw(|f| app.render(f)).unwrap();
        assert_eq!(
            app.last_hits.input_area.height,
            crate::ui::CHAT_INPUT_MAX_ROWS + 2
        );
        let rendered = buffer_to_text(terminal.backend().buffer(), 120, 40);
        assert!(
            rendered.contains("第 12/12 行"),
            "missing overflow line counter:\n{rendered}"
        );
    }

    #[test]
    fn resume_picker_shows_current_dir_scope_timestamps_and_switches_to_all() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        app.editor.set_text("/resume");
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let cur_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
        assert!(cur_view.contains("当前目录"), "missing current dir tab:\n{cur_view}");
        assert!(cur_view.contains("全部会话"), "missing all sessions tab:\n{cur_view}");
        assert!(cur_view.contains("[当前目录]"), "missing current dir badge:\n{cur_view}");

        // Tab 切换到全部会话，应能看到其他目录的会话
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        terminal.draw(|f| app.render(f)).unwrap();
        let all_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
        assert!(
            all_view.contains("Android SystemUI"),
            "switching to All scope should show other-dir session:\n{all_view}"
        );
    }

    #[test]
    fn busy_bottom_status_renders_sai_companion_and_status_label() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        app.busy = true;
        app.transcript.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
        app.transcript.append_thought("checking animation state");

        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let view = buffer_to_text(terminal.backend().buffer(), 120, 36);
        assert!(
            view.contains('‿'),
            "busy bottom pulse line should render sai companion face:\n{view}"
        );
        assert!(
            view.contains("思考中"),
            "busy bottom pulse line should reflect active thinking status:\n{view}"
        );
        assert!(
            view.contains("3s"),
            "busy bottom pulse line should show calm integer seconds:\n{view}"
        );
        assert!(
            view.contains("Esc 中断"),
            "busy bottom pulse line should keep right-aligned Esc hint:\n{view}"
        );
    }
}

