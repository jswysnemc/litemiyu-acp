//! 鼠标滚轮、悬停高亮、滚动条拖动、划词复制与点击交互。

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::clipboard;
use crate::ui::{HoverTarget, TextSelection};

use super::App;

impl App {
    /// 处理终端鼠标事件（滚动、悬停高亮、点击与划词拖拽）。
    ///
    /// 参数:
    /// - `mouse`: crossterm 鼠标事件
    ///
    /// 返回:
    /// - 界面状态实际发生变化（需要重绘）时返回 `true`，空白区移动等无效事件返回 `false`
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.picker.is_some() || self.permission.is_some() || self.elicitation.is_some() {
            let cleared = self.hover != HoverTarget::None;
            self.hover = HoverTarget::None;
            if self.picker.is_some() {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        if let Some(p) = &mut self.picker {
                            p.move_up();
                        }
                        return true;
                    }
                    MouseEventKind::ScrollDown => {
                        if let Some(p) = &mut self.picker {
                            p.move_down();
                        }
                        return true;
                    }
                    _ => {}
                }
            }
            return cleared;
        }

        match mouse.kind {
            MouseEventKind::Moved => {
                if self.dragging || self.dragging_scrollbar {
                    return false;
                }
                let next = self.resolve_hover_target(mouse.column, mouse.row);
                if next != self.hover {
                    self.hover = next;
                    return true;
                }
                false
            }
            MouseEventKind::ScrollUp => {
                self.selection = None;
                self.hover = HoverTarget::None;
                self.dragging = false;
                self.scroll_up(3);
                true
            }
            MouseEventKind::ScrollDown => {
                self.selection = None;
                self.hover = HoverTarget::None;
                self.dragging = false;
                self.scroll_down(3);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                self.hover = HoverTarget::None;
                // 1. 点击「↓ 回到底部」按钮：立即回到最底部并恢复自动跟随
                if rect_contains(self.last_hits.bottom_button, col, row) {
                    self.selection = None;
                    self.dragging = false;
                    self.dragging_scrollbar = false;
                    self.jump_to_bottom();
                    return true;
                }
                // 2. 点击/按住正文右侧滚动条：立即按比例定位并开始拖动
                if rect_contains(self.last_hits.scrollbar, col, row) {
                    self.selection = None;
                    self.dragging = false;
                    self.dragging_scrollbar = true;
                    self.scroll_to_scrollbar_row(row);
                    return true;
                }
                let zone = self.selection_zone(col, row);
                self.selection = Some(TextSelection {
                    anchor: (col, row),
                    head: (col, row),
                    zone,
                });
                self.dragging = false;
                self.dragging_scrollbar = false;
                true
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.reveal_fold_at(mouse.column, mouse.row, true)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                if self.dragging_scrollbar {
                    self.scroll_to_scrollbar_row(row);
                    return true;
                }
                if let Some(sel) = &mut self.selection {
                    if (col, row) != sel.anchor {
                        self.dragging = true;
                    }
                    sel.head = (col, row);
                    return true;
                }
                false
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                if self.dragging_scrollbar {
                    self.dragging_scrollbar = false;
                    self.scroll_to_scrollbar_row(row);
                    return true;
                }
                if self.dragging {
                    self.dragging = false;
                    if let Some(mut sel) = self.selection {
                        sel.head = (col, row);
                        self.selection = Some(sel);
                        let extracted = self.last_hits.extract_selection(sel);
                        if !extracted.is_empty() {
                            let chars = extracted.chars().count();
                            clipboard::copy_text(&extracted);
                            self.toast(format!("已复制选中内容 ({chars} 字符)"), false);
                        } else {
                            self.selection = None;
                        }
                    }
                    return true;
                }

                // 单击（非拖拽）：取消选区并处理按钮/链接/折叠/侧边栏点击
                self.selection = None;
                self.handle_left_click(col, row);
                true
            }
            _ => false,
        }
    }

    /// 根据鼠标坐标解析紧凑可交互组件的悬停目标（排除正文大块与折叠区）。
    ///
    /// 参数:
    /// - `col`: 终端列坐标
    /// - `row`: 终端行坐标
    ///
    /// 返回:
    /// - 命中的紧凑 `HoverTarget`，未命中返回 `HoverTarget::None`
    fn resolve_hover_target(&self, col: u16, row: u16) -> HoverTarget {
        // 1. 底部「回到底部」按钮与底栏用量单元格
        if rect_contains(self.last_hits.bottom_button, col, row) {
            return HoverTarget::BottomButton;
        }
        if rect_contains(self.last_hits.usage_cell, col, row) {
            return HoverTarget::UsageCell;
        }
        // 2. 右侧边栏紧凑交互项（后台任务、上下文进度区、跳转消息、最近会话）
        let side = &self.last_hits.sidebar;
        if side.area.width > 0 && col >= side.area.x && col < side.area.right() {
            if let Some((_, entry)) = side.subagent_rows.iter().find(|(screen_y, _)| *screen_y == row) {
                return HoverTarget::SidebarSubagent(*entry);
            }
            if let Some((_, eid)) = side.bg_rows.iter().find(|(y, _)| *y == row) {
                return HoverTarget::SidebarBackground(*eid);
            }
            if rect_contains(side.context_area, col, row) {
                return HoverTarget::SidebarContext;
            }
            if let Some((_, eid)) = side.jump_rows.iter().find(|(y, _)| *y == row) {
                return HoverTarget::SidebarJump(*eid);
            }
            if let Some((_, sid)) = side.session_rows.iter().find(|(y, _)| *y == row) {
                return HoverTarget::SidebarSession(sid.clone());
            }
            return HoverTarget::None;
        }
        // 3. 【正文】【折叠提示】仅命中实际提示行，避免覆盖工具标题与滚动条
        if rect_contains(self.last_hits.body_text, col, row)
            && let Some(&(entry, section)) = self.last_hits.fold_rows.get(&row)
        {
            return HoverTarget::Fold(entry, section);
        }
        if col >= self.last_hits.body.x && col < self.last_hits.body.right() {
            let rel_col = col.saturating_sub(self.last_hits.body.x);
            if let Some((_, _, _, links)) = self
                .last_hits
                .body_rows
                .iter()
                .find(|(y, _, _, _)| *y == row)
                && let Some((a, b, _)) = links
                    .iter()
                    .find(|(a, b, _)| rel_col >= *a && rel_col < *b)
            {
                return HoverTarget::BodyLink {
                    row,
                    col_start: self.last_hits.body.x.saturating_add(*a),
                    col_end: self.last_hits.body.x.saturating_add(*b),
                };
            }
        }
        HoverTarget::None
    }

    fn selection_zone(&self, col: u16, row: u16) -> ratatui::layout::Rect {
        // 1. 侧边栏区域（包含左侧分隔竖线列，均自动限定到纯文字区，不带入 `│`）
        let side = self.last_hits.sidebar.area;
        if side.width > 2 && col >= side.x && col < side.right() {
            return ratatui::layout::Rect::new(
                side.x + 2,
                side.y,
                side.width.saturating_sub(2),
                side.height,
            );
        }
        // 2. 输入框区域：仅选中内部纯文字区，不带入圆角边框 `│`/`╭`/`╰`、`❯` 与迷你滚动条
        let input_box = self.last_hits.input_area;
        if input_box.width > 0
            && self.last_hits.input_text_area.width > 0
            && rect_contains(input_box, col, row)
        {
            return self.last_hits.input_text_area;
        }
        // 3. 主对话栏正文区（含左右留白边距、顶部空行与状态行起手拖拽），严格限定在纯文字区，绝不带入右侧滚动条 `┃`/`│` 与边栏竖线 `│`
        let body_text = self.last_hits.body_text;
        let main_area = self.last_hits.main_area;
        let in_main_x = if main_area.width > 0 {
            col >= main_area.x && col < main_area.right()
        } else {
            side.width == 0 || col < side.x
        };
        if body_text.width > 0 && in_main_x && (input_box.height == 0 || row < input_box.y) {
            return body_text;
        }
        // 4. 其余区域兜底（如底栏/首页），同样横向限定在主栏内避免越过侧边栏竖线
        if main_area.width > 0 && main_area.height > 0 {
            return main_area;
        }
        let rows = self.last_hits.screen_snapshot.height.max(1);
        let cols = self.last_hits.screen_snapshot.width.max(1);
        ratatui::layout::Rect::new(0, 0, cols, rows)
    }

    fn handle_left_click(&mut self, col: u16, row: u16) {
        if let HoverTarget::SidebarSubagent(entry) = self.resolve_hover_target(col, row) {
            self.transcript.set_entry_expanded(entry, true);
            if let Some(start) = self.last_hits.entry_starts.get(&entry) {
                self.scroll = *start;
                self.auto_follow = false;
            }
            return;
        }
        if self.reveal_fold_at(col, row, false) {
            return;
        }
        // 0. 点击「回到底部」/ 底栏用量格 / 侧边栏上下文进度条
        if rect_contains(self.last_hits.bottom_button, col, row) {
            self.jump_to_bottom();
            return;
        }
        if rect_contains(self.last_hits.usage_cell, col, row)
            || rect_contains(self.last_hits.sidebar.context_area, col, row)
        {
            self.open_status_picker();
            return;
        }

        // 1. 点击侧边栏后台任务 / 跳转消息 / 切换最近会话
        if self.last_hits.sidebar.area.width > 0
            && col >= self.last_hits.sidebar.area.x
            && col < self.last_hits.sidebar.area.right()
        {
            if let Some((_, eid)) = self
                .last_hits
                .sidebar
                .bg_rows
                .iter()
                .find(|(y, _)| *y == row)
            {
                if let Some(&target_row) = self.last_hits.entry_starts.get(eid) {
                    self.auto_follow = false;
                    self.scroll = target_row;
                }
                return;
            }
            if let Some((_, eid)) = self
                .last_hits
                .sidebar
                .jump_rows
                .iter()
                .find(|(y, _)| *y == row)
            {
                if let Some(&target_row) = self.last_hits.entry_starts.get(eid) {
                    self.auto_follow = false;
                    self.scroll = target_row;
                }
                return;
            }
            if let Some((_, sid)) = self
                .last_hits
                .sidebar
                .session_rows
                .iter()
                .find(|(y, _)| *y == row)
                .cloned()
            {
                self.load_session(sid);
                return;
            }
        }

        // 2. 点击正文区域：优先打开链接，其次展开/折叠思考与工具块
        if col >= self.last_hits.body.x && col < self.last_hits.body.right() {
            let rel_col = col.saturating_sub(self.last_hits.body.x);
            if let Some((_, eid, clickable, links)) = self
                .last_hits
                .body_rows
                .iter()
                .find(|(y, _, _, _)| *y == row)
                .cloned()
            {
                if let Some((_, _, url)) = links
                    .iter()
                    .find(|(a, b, _)| rel_col >= *a && rel_col < *b)
                {
                    clipboard::open_target(url);
                    self.toast(format!("已打开 {url}"), false);
                    return;
                }
                if clickable {
                    self.transcript.toggle_entry(eid);
                }
            }
        }
    }


    /// 【正文】【展开折叠行】按屏幕坐标展开分区，`all` 决定是否全部展开，返回是否命中。
    fn reveal_fold_at(&mut self, col: u16, row: u16, all: bool) -> bool {
        if !rect_contains(self.last_hits.body_text, col, row) {
            return false;
        }
        let Some(&(entry, section)) = self.last_hits.fold_rows.get(&row) else { return false };
        // 1. 固定当前视口起点，新增内容不会把点击位置推离屏幕
        if self.auto_follow {
            self.scroll = self.last_hits.total_body_rows.saturating_sub(self.last_hits.visible_body_rows);
        }
        self.auto_follow = false;
        self.selection = None;
        self.hover = HoverTarget::None;
        self.transcript.reveal_fold(entry, section, all);
        true
    }

    fn jump_to_bottom(&mut self) {
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        self.scroll = max_scroll;
        self.auto_follow = true;
    }

    fn scroll_to_scrollbar_row(&mut self, row: u16) {
        let bar = self.last_hits.scrollbar;
        if bar.height == 0 {
            return;
        }
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        if max_scroll == 0 {
            self.scroll = 0;
            self.auto_follow = true;
            return;
        }
        let clamped = row.clamp(bar.y, bar.bottom().saturating_sub(1));
        let rel = clamped.saturating_sub(bar.y) as f64;
        let span = bar.height.saturating_sub(1).max(1) as f64;
        let target = ((rel / span) * max_scroll as f64).round() as usize;
        self.scroll = target.min(max_scroll);
        self.auto_follow = self.scroll >= max_scroll;
    }


    pub(super) fn scroll_up(&mut self, lines: usize) {
        self.auto_follow = false;
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        let max_scroll = self
            .last_hits
            .total_body_rows
            .saturating_sub(self.last_hits.visible_body_rows);
        self.scroll = (self.scroll + lines).min(max_scroll);
        if self.scroll >= max_scroll {
            self.auto_follow = true;
        }
    }

}

fn rect_contains(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.right()
        && row >= rect.y
        && row < rect.bottom()
}

