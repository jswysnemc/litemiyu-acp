//! 键盘快捷键分发、括号/剪贴板粘贴、图片附件与外部编辑器协同。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::clipboard::{self, ClipboardContent, LoadedImage};
use crate::completion::CompletionKind;
use crate::input::AtomKind;
use crate::paste::WindowsPasteOutcome;

use crate::ui::overlay::PickerKind;

use super::{App, PendingImage};

impl App {
    #[cfg(test)]
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_queued(key, false);
    }

    pub fn handle_key_queued(&mut self, key: KeyEvent, has_more_queued_keys: bool) {
        // Windows crossterm 会同时上报 Release 事件，Kitty 增强协议可能上报裸修饰键，统一忽略
        if key.kind == KeyEventKind::Release || matches!(key.code, KeyCode::Modifier(_)) {
            return;
        }

        // 1. 权限审批抽屉优先
        if self.permission.is_some() {
            self.windows_paste.reset();
            self.handle_permission_key(key);
            return;
        }

        // 2. 模态选择器优先
        if self.picker.is_some() {
            self.windows_paste.reset();
            self.handle_picker_key(key);
            return;
        }

        // 3. 全局快捷键
        if is_ctrl_char(key.code, key.modifiers, 'g')
            || (matches!(key.code, KeyCode::Char('g' | 'G'))
                && key.modifiers == KeyModifiers::ALT)
        {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.external_editor_pending = true;
            return;
        }
        if is_ctrl_char(key.code, key.modifiers, 'a') {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.editor.move_to_start();
            return;
        }
        if is_ctrl_char(key.code, key.modifiers, 'e') {
            self.windows_paste.reset();
            self.selection = None;
            self.dragging = false;
            self.editor.move_to_end();
            return;
        }
        if is_ctrl_char(key.code, key.modifiers, 'v')
            || (matches!(key.code, KeyCode::Char('v' | 'V'))
                && key.modifiers == KeyModifiers::ALT)
        {
            self.windows_paste.reset();
            self.paste_from_clipboard(false);
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                // 若存在鼠标划词选区，Ctrl+C 直接复制选区并取消高亮
                if let Some(sel) = self.selection.take() {
                    let text = self.last_hits.extract_selection(sel);
                    if !text.is_empty() {
                        let count = text.chars().count();
                        clipboard::copy_text(&text);
                        self.toast(format!("已复制选中内容 ({count} 字符)"), false);
                        return;
                    }
                }
                if self.is_generating() {
                    self.cancel_turn();
                } else if !self.editor.is_empty() || !self.pending_images.is_empty() {
                    self.editor.clear();
                    self.pending_images.clear();
                    self.toast("已清空输入框（再按一次 Ctrl+C 退出）", false);
                    self.last_ctrl_c = Some(Instant::now());
                } else if self
                    .last_ctrl_c
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
                {
                    self.should_quit = true;
                } else {
                    self.last_ctrl_c = Some(Instant::now());
                    self.toast("再按一次 Ctrl+C 退出", false);
                }
                return;
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                if self.editor.is_empty() {
                    self.should_quit = true;
                } else if let Some(atom) = self.editor.delete()
                    && atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
                return;
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.show_sidebar = !self.show_sidebar;
                self.save_prefs();
                self.toast(
                    if self.show_sidebar {
                        "已开启右侧边栏"
                    } else {
                        "已隐藏右侧边栏"
                    },
                    false,
                );
                return;
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.all_expanded = !self.all_expanded;
                self.transcript.toggle_all_details(self.all_expanded);
                self.toast(
                    if self.all_expanded {
                        "已展开全部思考与工具详情"
                    } else {
                        "已折叠全部思考与工具详情"
                    },
                    false,
                );
                return;
            }
            (KeyCode::BackTab, _) => {
                self.windows_paste.reset();
                self.cycle_mode();
                return;
            }
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                self.windows_paste.reset();
                self.cycle_mode();
                return;
            }
            (KeyCode::PageUp, _) => {
                self.windows_paste.reset();
                self.scroll_up(12);
                return;
            }
            (KeyCode::PageDown, _) => {
                self.windows_paste.reset();
                self.scroll_down(12);
                return;
            }
            (KeyCode::Up, KeyModifiers::ALT) => {
                self.windows_paste.reset();
                self.scroll_up(3);
                return;
            }
            (KeyCode::Down, KeyModifiers::ALT) => {
                self.windows_paste.reset();
                self.scroll_down(3);
                return;
            }
            (KeyCode::End, m) if self.editor.is_empty() || m.contains(KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.auto_follow = true;
                return;
            }
            (KeyCode::Home, m) if m.contains(KeyModifiers::CONTROL) => {
                self.windows_paste.reset();
                self.auto_follow = false;
                self.scroll = 0;
                return;
            }
            (KeyCode::Esc, _) => {
                self.windows_paste.reset();
                // 优先收起当前弹出的 `/ # @` 补全菜单，不误清空用户输入
                if self.active_completion_menu().is_some() {
                    self.completion_dismissed_text = Some(self.editor.text().to_string());
                    self.completion_selected = 0;
                    return;
                }
                if self.selection.take().is_some() {
                    self.dragging = false;
                } else if self.is_generating() {
                    self.cancel_turn();
                } else if !self.editor.is_empty() || !self.pending_images.is_empty() {
                    self.editor.clear();
                    self.pending_images.clear();
                }
                return;
            }
            _ => {}
        }

        // 3.5 Windows 无括号多行粘贴检测与回放吞并（Linux/macOS 原生支持 Bracketed Paste，避免误拦截快打）
        if cfg!(windows) {
            match self.windows_paste.handle_key(
                key,
                self.editor.text_before_cursor(),
                has_more_queued_keys,
                Instant::now(),
                clipboard::read_clipboard_text,
            ) {
                WindowsPasteOutcome::ConsumedReplay => {
                    self.windows_paste.drain_console_replay();
                    return;
                }
                WindowsPasteOutcome::ReplacePrefixWithPaste {
                    prefix_bytes,
                    full_text,
                } => {
                    self.editor.delete_bytes_before_cursor(prefix_bytes);
                    self.insert_pasted_text(full_text);
                    self.windows_paste.drain_console_replay();
                    return;
                }
                WindowsPasteOutcome::InsertBurstChar(ch) => {
                    self.editor.insert_char(ch);
                    return;
                }
                WindowsPasteOutcome::NotPaste => {}
            }
        }

        // 4. 统一输入补全菜单导航（`/` 命令、`#` Skills、`@` 文件）
        if let Some(menu) = self.active_completion_menu() {
            let len = menu.items.len();
            let sel = self.completion_selected.min(len.saturating_sub(1));
            match key.code {
                KeyCode::Up => {
                    self.completion_selected = if sel == 0 { len - 1 } else { sel - 1 };
                    return;
                }
                KeyCode::Down => {
                    self.completion_selected = (sel + 1) % len;
                    return;
                }
                KeyCode::Tab => {
                    if let Some(item) = menu.items.get(sel) {
                        self.editor
                            .replace_before_cursor(menu.replace_start, &item.insert_text);
                        self.completion_selected = 0;
                    }
                    return;
                }
                KeyCode::Enter if key.modifiers.is_empty() => {
                    if let Some(item) = menu.items.get(sel) {
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
                    return;
                }
                _ => {}
            }
        }

        // 5. 普通输入框编辑（开始输入时清除旧的划词高亮）
        if self.selection.is_some() {
            self.selection = None;
            self.dragging = false;
        }
        let input_w = self.last_hits.input_text_width.max(16);
        match (key.code, key.modifiers) {
            (KeyCode::Enter, m)
                if m.contains(KeyModifiers::SHIFT)
                    || m.contains(KeyModifiers::ALT)
                    || m.contains(KeyModifiers::CONTROL) =>
            {
                self.editor.insert_char('\n');
            }
            (KeyCode::Char('\n'), _)
            | (KeyCode::Char('j' | 'J'), KeyModifiers::CONTROL) => {
                self.editor.insert_char('\n');
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.windows_paste.reset();
                let (_raw, expanded) = self.editor.take_and_record();
                if !expanded.trim().is_empty() || !self.pending_images.is_empty() {
                    self.submit_text(expanded);
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(atom) = self.editor.backspace()
                    && atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
            }
            (KeyCode::Delete, _) => {
                if let Some(atom) = self.editor.delete()
                    && atom.kind == AtomKind::Image {
                        self.prune_pending_images();
                    }
            }
            (KeyCode::Left, _) => self.editor.move_left(),
            (KeyCode::Right, _) => self.editor.move_right(),
            (KeyCode::Home, _) => self.editor.move_home(),
            (KeyCode::End, _) => self.editor.move_end(),
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                self.editor.delete_word_back();
                self.prune_pending_images();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.editor.delete_to_start();
                self.prune_pending_images();
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.editor.delete_to_end();
                self.prune_pending_images();
            }
            (KeyCode::Up, _) => {
                if !self.editor.move_up(input_w) {
                    self.editor.history_prev();
                }
            }
            (KeyCode::Down, _) => {
                if !self.editor.move_down(input_w) {
                    self.editor.history_next();
                }
            }
            (KeyCode::Char(c), m)
                if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
            {
                self.editor.insert_char(c);
            }
            _ => {}
        }
    }

    /// 准备写入外部编辑器（`$EDITOR` / `notepad.exe`）的输入正文（展开大段粘贴块）。
    pub fn prepare_external_editor_buffer(&self) -> String {
        self.editor.expanded_text()
    }

    /// 外部编辑器退出后，将编辑后的正文替换回输入框并同步待发送图片附件。
    pub fn apply_external_editor_result(&mut self, result: Result<String, String>) {
        self.windows_paste.reset();
        match result {
            Ok(edited) => {
                let prev = self.editor.expanded_text();
                let markers: Vec<String> = self
                    .pending_images
                    .iter()
                    .map(|img| img.label.clone())
                    .collect();
                self.editor.replace_from_external(edited.clone(), &markers);
                self.prune_pending_images();
                if edited == prev {
                    self.toast("外部编辑器未修改内容", false);
                } else {
                    let chars = edited.chars().count();
                    self.toast(format!("已从外部编辑器替换输入 ({chars} 字符)"), false);
                }
            }
            Err(err) => {
                self.toast(err, true);
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        self.windows_paste.reset();
        if let Some(picker) = &mut self.picker {
            if picker.kind != PickerKind::Help {
                picker.query.push_str(text.trim());
                picker.selected = 0;
            }
            return;
        }
        if self.permission.is_some() {
            return;
        }
        // 部分终端在剪贴板为图片时会发出空 Bracketed Paste 事件，趁机主动探测系统剪贴板图片
        if text.is_empty() {
            self.paste_from_clipboard(true);
            return;
        }
        // 拖拽图片进终端或粘贴图片文件路径时自动加载为图片芯片
        if let Some(imgs) = clipboard::try_load_pasted_image_paths(&text) {
            self.attach_images(imgs);
            return;
        }
        self.insert_pasted_text(text);
    }

    fn insert_pasted_text(&mut self, text: String) {
        if let Some(imgs) = clipboard::try_load_pasted_image_paths(&text) {
            self.attach_images(imgs);
            return;
        }
        if let Some((_marker, lines, chars)) = self.editor.insert_pasted_text(&text) {
            self.toast(
                format!("已粘贴为原子文本块 ({lines} 行 / {chars} 字，退格一键删除)"),
                false,
            );
        }
    }

    pub(super) fn paste_from_clipboard(&mut self, image_only: bool) {
        match clipboard::read_clipboard(image_only) {
            ClipboardContent::Images(imgs) => self.attach_images(imgs),
            ClipboardContent::Text(s) => self.insert_pasted_text(s),
            ClipboardContent::Empty => {
                self.toast("剪贴板中没有图片或文字", true);
            }
        }
    }

    fn prune_pending_images(&mut self) {
        let current = self.editor.text();
        self.pending_images
            .retain(|img| current.contains(&img.label));
    }

    pub(super) fn attach_images(&mut self, imgs: Vec<LoadedImage>) {
        if imgs.is_empty() {
            return;
        }
        let mut labels = Vec::new();
        let mut total_kb = 0usize;
        for img in imgs {
            self.image_seq += 1;
            let label = format!("[图片 {}]", self.image_seq);
            total_kb += img.byte_len.div_ceil(1024);
            self.editor.insert_image_atom(label.clone());
            labels.push(label.clone());
            self.pending_images.push(PendingImage { label, image: img });
        }
        self.toast(
            format!("已附加 {} ({total_kb} KB，退格一键删除)", labels.join(" ")),
            false,
        );
    }

}

fn is_ctrl_char(code: KeyCode, modifiers: KeyModifiers, target: char) -> bool {
    let lower = target.to_ascii_lowercase();
    let upper = target.to_ascii_uppercase();
    if modifiers == KeyModifiers::CONTROL
        && matches!(code, KeyCode::Char(c) if c == lower || c == upper)
    {
        return true;
    }
    // 兼容部分终端直接发送 ASCII 控制字符（Ctrl+A = 0x01 .. Ctrl+Z = 0x1A）
    if lower.is_ascii_lowercase() {
        let ctrl_ch = ((lower as u8) - b'a' + 1) as char;
        if matches!(code, KeyCode::Char(c) if c == ctrl_ch)
            && !modifiers.contains(KeyModifiers::ALT)
        {
            return true;
        }
    }
    false
}


#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn ctrl_a_ctrl_e_and_ctrl_g_shortcuts_work_on_multiline_input() {
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        app.editor.set_text("line1\nline2\nline3");

        // Ctrl+A 回到整个输入框最开头（即便在多行末尾）
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text_before_cursor(), "");

        // Ctrl+E 回到整个输入框最末尾
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(app.editor.text_before_cursor(), "line1\nline2\nline3");

        // 兼容 ASCII 控制字符形式（0x01 / 0x05 / 0x07）
        app.handle_key(KeyEvent::new(KeyCode::Char('\x01'), KeyModifiers::NONE));
        assert_eq!(app.editor.text_before_cursor(), "");
        app.handle_key(KeyEvent::new(KeyCode::Char('\x05'), KeyModifiers::NONE));
        assert_eq!(app.editor.text_before_cursor(), "line1\nline2\nline3");

        // Ctrl+G 触发外部编辑器挂起标志，回调替换后光标落在末尾
        assert!(!app.external_editor_pending);
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(app.external_editor_pending);
        app.external_editor_pending = false;

        let initial = app.prepare_external_editor_buffer();
        assert_eq!(initial, "line1\nline2\nline3");
        app.apply_external_editor_result(Ok("外部编辑后的新内容\n第二行".into()));
        assert_eq!(app.editor.text(), "外部编辑后的新内容\n第二行");
        assert_eq!(app.editor.text_before_cursor(), "外部编辑后的新内容\n第二行");
    }

    #[test]
    fn shift_enter_and_lf_insert_newline_without_submitting() {
        let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
        let initial_entries = app.transcript.entries.len();
        app.editor.set_text("第一行");

        // 1. Kitty 键盘增强协议上报的 Shift+Enter 插入换行，不提交消息
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.editor.text(), "第一行\n");
        assert_eq!(app.transcript.entries.len(), initial_entries);

        // 2. 终端直接发送的裸 LF（\n）同样插入换行，不提交消息
        app.handle_key(KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE));
        assert_eq!(app.editor.text(), "第一行\n\n");
        assert_eq!(app.transcript.entries.len(), initial_entries);

        // 3. 裸修饰键事件被忽略，不清空输入
        app.handle_key(KeyEvent::new(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
        ));
        assert_eq!(app.editor.text(), "第一行\n\n");
    }
}
