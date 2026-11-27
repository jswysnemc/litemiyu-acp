//! 模态选择器、权限审批抽屉、代理提问表单与输入补全浮层的鼠标交互分发。

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use serde_json::json;

use crate::clipboard;
use crate::completion::CompletionKind;
use crate::ui::overlay::PickerKind;
use crate::ui::{HoverTarget, OverlayFooterAction};

use super::App;
use super::mouse::rect_contains;

impl App {
    /// 【浮层】【鼠标分发】优先处理模态弹窗、底部审批/提问抽屉及输入补全菜单的鼠标悬停、点击与滚轮事件。
    ///
    /// 参数:
    /// - `mouse`: crossterm 鼠标事件
    ///
    /// 返回:
    /// - `Some(redraw)` 表示事件已被浮层消费（`redraw` 指示是否需要重绘）
    /// - `None` 表示当前无活跃浮层拦截，交由主界面鼠标逻辑继续处理
    pub(super) fn handle_overlay_mouse(&mut self, mouse: MouseEvent) -> Option<bool> {
        if self.picker.is_some() {
            return Some(self.handle_picker_mouse(mouse));
        }
        if self.permission.is_some() {
            return self.handle_permission_mouse(mouse);
        }
        if self.elicitation.is_some() {
            return self.handle_elicitation_mouse(mouse);
        }
        if self.active_completion_menu().is_some() {
            return self.handle_completion_mouse(mouse);
        }
        None
    }

    /// 处理模态选择器（上下文状态/模型/思考等级/模式/历史会话/主题/队列/帮助）的鼠标交互。
    fn handle_picker_mouse(&mut self, mouse: MouseEvent) -> bool {
        let had_selection = self.selection.is_some();
        self.selection = None;
        self.dragging = false;
        self.dragging_scrollbar = false;

        let col = mouse.column;
        let row = mouse.row;
        let hits = self.last_hits.overlay.picker.clone();
        let footer_hit = hits
            .footer_actions
            .iter()
            .find(|(rect, _)| rect_contains(*rect, col, row))
            .map(|(_, a)| *a);

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.hover = HoverTarget::None;
                if let Some(p) = &mut self.picker {
                    p.move_up();
                }
                self.preview_selected_theme();
                true
            }
            MouseEventKind::ScrollDown => {
                self.hover = HoverTarget::None;
                if let Some(p) = &mut self.picker {
                    p.move_down();
                }
                self.preview_selected_theme();
                true
            }
            MouseEventKind::Moved => {
                let next_hover = footer_hit.map_or(HoverTarget::None, HoverTarget::OverlayAction);
                let hover_changed = next_hover != self.hover;
                self.hover = next_hover;
                let mut sel_changed = false;
                if let Some((_, idx)) = hits
                    .items
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && let Some(p) = &mut self.picker
                    && p.kind != PickerKind::Help
                    && p.selected != *idx
                {
                    p.selected = *idx;
                    sel_changed = true;
                }
                if sel_changed {
                    self.preview_selected_theme();
                }
                had_selection || hover_changed || sel_changed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let next_hover = footer_hit.map_or(HoverTarget::None, HoverTarget::OverlayAction);
                let hover_changed = next_hover != self.hover;
                self.hover = next_hover;
                if let Some((_, idx)) = hits
                    .items
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && let Some(p) = &mut self.picker
                    && p.kind != PickerKind::Help
                    && p.selected != *idx
                {
                    p.selected = *idx;
                    self.preview_selected_theme();
                    return true;
                }
                had_selection || hover_changed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.hover = HoverTarget::None;
                // 1. 底部边框动作按钮（Enter 确认 / Tab 切换范围 / Esc/点外部 关闭）
                if let Some(action) = footer_hit {
                    match action {
                        OverlayFooterAction::Confirm => self.confirm_picker_selection(),
                        OverlayFooterAction::Close => self.dismiss_picker(),
                        OverlayFooterAction::ToggleResumeScope => {
                            if let Some(p) = &mut self.picker {
                                p.toggle_scope();
                            }
                        }
                        _ => {}
                    }
                    return true;
                }
                // 2. `/resume` 顶部范围标签页单击切换
                if let Some((_, scope)) = hits
                    .resume_tabs
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && let Some(p) = &mut self.picker
                {
                    if p.scope != *scope {
                        p.scope = *scope;
                        p.selected = 0;
                    }
                    return true;
                }
                // 3. 单击列表项直接选中并确认执行
                if let Some((_, idx)) = hits
                    .items
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                {
                    if let Some(p) = &mut self.picker
                        && p.kind != PickerKind::Help
                    {
                        p.selected = *idx;
                        self.confirm_picker_selection();
                        return true;
                    }
                    return had_selection;
                }
                // 4. 点击弹窗外部空白处关闭弹窗
                if hits.popup.width > 0 && !rect_contains(hits.popup, col, row) {
                    self.dismiss_picker();
                    return true;
                }
                had_selection
            }
            _ => had_selection,
        }
    }

    /// 处理底部权限审批抽屉的鼠标悬停、底栏按钮、单击确认与抽屉内滚轮切换。
    fn handle_permission_mouse(&mut self, mouse: MouseEvent) -> Option<bool> {
        let col = mouse.column;
        let row = mouse.row;
        let hits = self.last_hits.overlay.permission.clone();
        let in_drawer = rect_contains(hits.area, col, row);

        if !in_drawer
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
        {
            return None;
        }

        let footer_hit = hits
            .footer_actions
            .iter()
            .find(|(rect, _)| rect_contains(*rect, col, row))
            .map(|(_, a)| *a);

        let redraw = match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.hover = HoverTarget::None;
                if let Some((req, sel)) = &mut self.permission
                    && !req.options.is_empty()
                {
                    let len = req.options.len();
                    *sel = if *sel == 0 { len - 1 } else { *sel - 1 };
                    return Some(true);
                }
                false
            }
            MouseEventKind::ScrollDown => {
                self.hover = HoverTarget::None;
                if let Some((req, sel)) = &mut self.permission
                    && !req.options.is_empty()
                {
                    *sel = (*sel + 1) % req.options.len();
                    return Some(true);
                }
                false
            }
            MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {
                let next_hover = footer_hit.map_or(HoverTarget::None, HoverTarget::OverlayAction);
                let hover_changed = next_hover != self.hover;
                self.hover = next_hover;
                let mut sel_changed = false;
                if let Some((_, idx)) = hits
                    .options
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && let Some((_, sel)) = &mut self.permission
                    && *sel != *idx
                {
                    *sel = *idx;
                    sel_changed = true;
                }
                hover_changed || sel_changed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.hover = HoverTarget::None;
                if let Some(action) = footer_hit {
                    match action {
                        OverlayFooterAction::Confirm => {
                            let opt_id = self
                                .permission
                                .as_ref()
                                .and_then(|(req, sel)| req.options.get(*sel))
                                .map(|o| o.option_id.clone());
                            self.answer_permission(opt_id);
                        }
                        OverlayFooterAction::Close => {
                            let opt_id = self
                                .permission
                                .as_ref()
                                .and_then(|(req, _)| {
                                    req.options.iter().find(|o| o.kind.contains("reject"))
                                })
                                .map(|o| o.option_id.clone());
                            self.answer_permission(opt_id);
                        }
                        _ => {}
                    }
                    return Some(true);
                }
                if let Some((_, idx)) = hits
                    .options
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                {
                    let opt_id = self
                        .permission
                        .as_ref()
                        .and_then(|(req, _)| req.options.get(*idx))
                        .map(|o| o.option_id.clone());
                    self.answer_permission(opt_id);
                    return Some(true);
                }
                false
            }
            _ => false,
        };
        Some(redraw)
    }

    /// 处理 `elicitation/create` 代理提问表单抽屉的鼠标悬停、底栏按钮、页签切题与选项点击。
    fn handle_elicitation_mouse(&mut self, mouse: MouseEvent) -> Option<bool> {
        let col = mouse.column;
        let row = mouse.row;
        let hits = self.last_hits.overlay.elicitation.clone();
        let in_drawer = rect_contains(hits.area, col, row);

        if !in_drawer
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
        {
            return None;
        }

        let footer_hit = hits
            .footer_actions
            .iter()
            .find(|(rect, _)| rect_contains(*rect, col, row))
            .map(|(_, a)| *a);

        let redraw = match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.hover = HoverTarget::None;
                self.step_elicitation_cursor(false)
            }
            MouseEventKind::ScrollDown => {
                self.hover = HoverTarget::None;
                self.step_elicitation_cursor(true)
            }
            MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {
                let next_hover = footer_hit.map_or(HoverTarget::None, HoverTarget::OverlayAction);
                let hover_changed = next_hover != self.hover;
                self.hover = next_hover;
                let mut sel_changed = false;
                if let Some((_, opt_idx, _)) = hits
                    .options
                    .iter()
                    .find(|(rect, _, _)| rect_contains(*rect, col, row))
                    && let Some(state) = &mut self.elicitation
                    && let Some(ans) = state.answers.get_mut(state.active_field)
                    && !ans.editing_text
                    && ans.cursor_opt != *opt_idx
                {
                    ans.cursor_opt = *opt_idx;
                    sel_changed = true;
                }
                hover_changed || sel_changed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.hover = HoverTarget::None;
                if let Some(action) = footer_hit {
                    match action {
                        OverlayFooterAction::Confirm => {
                            let is_url = self.elicitation.as_ref().is_some_and(|s| {
                                s.req.mode == "url" || s.req.fields.is_empty()
                            });
                            if is_url {
                                self.answer_elicitation("accept", Some(json!({})));
                            } else {
                                self.try_submit_elicitation();
                            }
                        }
                        OverlayFooterAction::OpenUrl => {
                            if let Some(url) = self
                                .elicitation
                                .as_ref()
                                .and_then(|s| s.req.url.clone())
                            {
                                clipboard::open_target(&url);
                                self.toast("已在浏览器打开确认链接", false);
                            }
                        }
                        OverlayFooterAction::Close => {
                            self.answer_elicitation("cancel", None);
                        }
                        _ => {}
                    }
                    return Some(true);
                }
                if let Some((_, tab_idx)) = hits
                    .tabs
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && let Some(state) = &mut self.elicitation
                    && *tab_idx < state.req.fields.len()
                {
                    state.active_field = *tab_idx;
                    return Some(true);
                }
                if let Some((_, opt_idx, is_other)) = hits
                    .options
                    .iter()
                    .find(|(rect, _, _)| rect_contains(*rect, col, row))
                {
                    self.click_elicitation_option(*opt_idx, *is_other);
                    return Some(true);
                }
                if let Some(url_rect) = hits.url_row
                    && rect_contains(url_rect, col, row)
                    && let Some(url) = self
                        .elicitation
                        .as_ref()
                        .and_then(|s| s.req.url.clone())
                {
                    clipboard::open_target(&url);
                    self.toast("已在浏览器打开确认链接", false);
                    return Some(true);
                }
                false
            }
            _ => false,
        };
        Some(redraw)
    }

    /// 滚轮上下移动当前提问字段的选项焦点。
    fn step_elicitation_cursor(&mut self, down: bool) -> bool {
        let Some(state) = &mut self.elicitation else {
            return false;
        };
        let Some(field) = state.req.fields.get(state.active_field) else {
            return false;
        };
        let Some(ans) = state.answers.get_mut(state.active_field) else {
            return false;
        };
        if ans.editing_text {
            return false;
        }
        let opt_len = match &field.kind {
            crate::acp::ElicitationFieldKind::SingleSelect(opts)
            | crate::acp::ElicitationFieldKind::MultiSelect { options: opts, .. } => opts.len(),
            crate::acp::ElicitationFieldKind::Boolean { .. } => 2,
            crate::acp::ElicitationFieldKind::Text { .. }
            | crate::acp::ElicitationFieldKind::Number { .. } => 0,
        };
        let row_count = opt_len + usize::from(field.other_key.is_some());
        if row_count == 0 {
            return false;
        }
        ans.cursor_opt = if down {
            (ans.cursor_opt + 1) % row_count
        } else if ans.cursor_opt == 0 {
            row_count - 1
        } else {
            ans.cursor_opt - 1
        };
        true
    }

    /// 处理输入框上方悬浮补全菜单（`/` 命令、`#` Skills、`@` 文件）的鼠标悬停、顶栏按钮、单击与滚轮。
    fn handle_completion_mouse(&mut self, mouse: MouseEvent) -> Option<bool> {
        let col = mouse.column;
        let row = mouse.row;
        let hits = self.last_hits.overlay.completion.clone();
        if hits.area.width == 0 {
            return None;
        }
        let in_menu = rect_contains(hits.area, col, row);
        let in_input = rect_contains(self.last_hits.input_area, col, row);

        if !in_menu {
            if matches!(self.hover, HoverTarget::OverlayAction(_)) {
                self.hover = HoverTarget::None;
            }
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) && !in_input {
                self.completion_dismissed_text = Some(self.editor.text().to_string());
                self.completion_selected = 0;
            }
            return None;
        }

        let Some(menu) = self.active_completion_menu() else {
            return None;
        };
        let len = menu.items.len();
        if len == 0 {
            return None;
        }
        let footer_hit = hits
            .footer_actions
            .iter()
            .find(|(rect, _)| rect_contains(*rect, col, row))
            .map(|(_, a)| *a);

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                let sel = self.completion_selected.min(len - 1);
                self.completion_selected = if sel == 0 { len - 1 } else { sel - 1 };
                Some(true)
            }
            MouseEventKind::ScrollDown => {
                let sel = self.completion_selected.min(len - 1);
                self.completion_selected = (sel + 1) % len;
                Some(true)
            }
            MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {
                let next_hover = footer_hit.map_or(HoverTarget::None, HoverTarget::OverlayAction);
                let hover_changed = next_hover != self.hover;
                self.hover = next_hover;
                let mut sel_changed = false;
                if let Some((_, idx)) = hits
                    .items
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                    && self.completion_selected != *idx
                {
                    self.completion_selected = *idx;
                    sel_changed = true;
                }
                Some(hover_changed || sel_changed)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.hover = HoverTarget::None;
                if let Some(action) = footer_hit {
                    match action {
                        OverlayFooterAction::AcceptCompletion => {
                            let idx = self.completion_selected.min(len - 1);
                            self.accept_completion_item(&menu, idx);
                        }
                        OverlayFooterAction::Close => {
                            self.completion_dismissed_text = Some(self.editor.text().to_string());
                            self.completion_selected = 0;
                        }
                        _ => {}
                    }
                    return Some(true);
                }
                if let Some((_, idx)) = hits
                    .items
                    .iter()
                    .find(|(rect, _)| rect_contains(*rect, col, row))
                {
                    self.accept_completion_item(&menu, *idx);
                    return Some(true);
                }
                Some(false)
            }
            _ => Some(false),
        }
    }

    fn accept_completion_item(&mut self, menu: &crate::completion::CompletionMenu, idx: usize) {
        if let Some(item) = menu.items.get(idx) {
            match menu.kind {
                CompletionKind::Command => {
                    let cmd = item
                        .command_name
                        .as_deref()
                        .map(|n| format!("/{n}"))
                        .unwrap_or_else(|| item.label.clone());
                    self.editor.clear();
                    self.completion_selected = 0;
                    self.submit_text(cmd);
                }
                CompletionKind::Skill | CompletionKind::File => {
                    self.editor
                        .replace_before_cursor(menu.replace_start, &item.insert_text);
                    self.completion_selected = 0;
                }
            }
        }
    }
}
