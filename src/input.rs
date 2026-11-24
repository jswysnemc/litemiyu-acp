//! 多行 UTF-8 安全输入框编辑器（支持中英文宽字精准光标定位、原子元素整块删除、上下行导航与历史记录翻页）。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 粘贴文本折叠为原子块的阈值（与 `sai` 保持一致）。
pub const LONG_PASTE_CHARS: usize = 200;
pub const LONG_PASTE_LINES: usize = 4;
pub const LONG_PASTE_LINE_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomKind {
    Image,
    PastedText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicToken {
    pub marker: String,
    pub kind: AtomKind,
    /// `PastedText` 保存完整正文，`Image` 为 `None`（由 `App.pending_images` 持有二进制载荷）。
    pub payload: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomSpan {
    pub start: usize,
    pub end: usize,
    pub kind: AtomKind,
}

#[derive(Debug, Clone, Copy)]
pub struct VisualLine {
    pub start: usize,
    pub end: usize,
}

pub struct InputEditor {
    text: String,
    cursor: usize, // byte index, always on grapheme boundary
    atoms: Vec<AtomicToken>,
    paste_seq: usize,
    history: Vec<(String, Vec<AtomicToken>)>,
    history_idx: Option<usize>,
    draft: (String, Vec<AtomicToken>),
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            atoms: Vec::new(),
            paste_seq: 0,
            history: Vec::new(),
            history_idx: None,
            draft: (String::new(), Vec::new()),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn text_before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.atoms.clear();
        self.history_idx = None;
    }

    #[cfg(test)]
    pub fn set_text(&mut self, s: impl Into<String>) {
        self.text = s.into();
        self.cursor = self.text.len();
        self.atoms.clear();
        self.history_idx = None;
    }

    /// 返回展开粘贴原子块后的完整正文（不清空当前缓冲，供外部编辑器 `Ctrl+G` 打开编辑）。
    pub fn expanded_text(&self) -> String {
        expand_atoms(&self.text, &self.atoms)
    }

    /// 用外部编辑器返回的正文替换输入内容，并保留仍存在于文本中的图片原子芯片。
    pub fn replace_from_external(&mut self, text: String, keep_image_markers: &[String]) {
        self.text = text;
        self.cursor = self.text.len();
        self.history_idx = None;
        self.atoms = keep_image_markers
            .iter()
            .filter(|marker| !marker.is_empty() && self.text.contains(marker.as_str()))
            .map(|marker| AtomicToken {
                marker: marker.clone(),
                kind: AtomKind::Image,
                payload: None,
            })
            .collect();
    }

    /// 返回当前输入中全部有效原子元素的有序字节区间。
    pub fn atom_spans(&self) -> Vec<AtomSpan> {
        let mut spans = Vec::new();
        for atom in &self.atoms {
            if atom.marker.is_empty() {
                continue;
            }
            for (start, _) in self.text.match_indices(&atom.marker) {
                let end = start + atom.marker.len();
                if !spans
                    .iter()
                    .any(|s: &AtomSpan| start < s.end && end > s.start)
                {
                    spans.push(AtomSpan {
                        start,
                        end,
                        kind: atom.kind,
                    });
                    break;
                }
            }
        }
        spans.sort_by_key(|s| s.start);
        spans
    }

    /// 取出当前文本并写入历史，返回 `(原始带标记文本, 展开粘贴正文后的完整文本)`。
    pub fn take_and_record(&mut self) -> (String, String) {
        self.prune_atoms();
        let raw = std::mem::take(&mut self.text);
        let atoms = std::mem::take(&mut self.atoms);
        let expanded = expand_atoms(&raw, &atoms);
        self.cursor = 0;
        self.history_idx = None;
        self.draft = (String::new(), Vec::new());

        if !expanded.trim().is_empty()
            && self
                .history
                .last()
                .map(|(t, _)| t.as_str())
                != Some(raw.as_str())
        {
            self.history.push((raw.clone(), atoms));
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        (raw, expanded)
    }

    pub fn insert_char(&mut self, ch: char) {
        self.snap_cursor_out_of_atom(true);
        self.history_idx = None;
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return;
        }
        self.snap_cursor_out_of_atom(true);
        self.history_idx = None;
        self.text.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
    }

    /// 插入一个图片原子芯片（单次 Backspace / Delete 整块移除）。
    pub fn insert_image_atom(&mut self, marker: String) {
        self.insert_atom(AtomicToken {
            marker,
            kind: AtomKind::Image,
            payload: None,
        });
    }

    /// 按阈值决定将粘贴文本直接写入或折叠为大段文本原子块。
    /// 返回 `Some((marker, lines, chars))` 表示已折叠为原子块。
    pub fn insert_pasted_text(&mut self, raw: &str) -> Option<(String, usize, usize)> {
        let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return None;
        }
        let chars = normalized.chars().count();
        let lines = normalized.lines().count().max(1);
        let max_line_chars = normalized
            .lines()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(chars);

        if chars <= LONG_PASTE_CHARS
            && lines < LONG_PASTE_LINES
            && max_line_chars <= LONG_PASTE_LINE_CHARS
        {
            self.insert_str(&normalized);
            return None;
        }

        self.paste_seq += 1;
        let marker = if lines > 1 {
            format!("[粘贴文本 {} · {}行/{}字]", self.paste_seq, lines, chars)
        } else {
            format!("[粘贴文本 {} · {}字]", self.paste_seq, chars)
        };
        self.insert_atom(AtomicToken {
            marker: marker.clone(),
            kind: AtomKind::PastedText,
            payload: Some(normalized),
        });
        Some((marker, lines, chars))
    }

    fn insert_atom(&mut self, atom: AtomicToken) {
        self.snap_cursor_out_of_atom(true);
        self.history_idx = None;
        // 紧贴另一个原子块右侧插入时补一个空格分隔，单个原子块末尾不留尾随空格以便一键退格删除
        let touches_prev_atom = self
            .atom_spans()
            .iter()
            .any(|s| s.end == self.cursor);
        if touches_prev_atom {
            self.text.insert(self.cursor, ' ');
            self.cursor += 1;
        }
        let len = atom.marker.len();
        self.text.insert_str(self.cursor, &atom.marker);
        self.cursor += len;
        self.atoms.push(atom);
    }

    /// 删掉光标前方指定字节数（供 Windows 无括号粘贴替换首行使用）。
    pub fn delete_bytes_before_cursor(&mut self, byte_len: usize) {
        if byte_len == 0 || self.cursor == 0 {
            return;
        }
        let target = self.cursor.saturating_sub(byte_len);
        let start = if self.text.is_char_boundary(target) {
            target
        } else {
            self.text[..target]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i)
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.prune_atoms();
    }

    /// 将 `[byte_start..cursor]` 区间替换为新文本并将光标移到替换结果末尾（用于 `/ # @` 补全插入）。
    ///
    /// 参数:
    /// - `byte_start`: 替换起始字节偏移（相对全文起点）
    /// - `replacement`: 写入的替换文本
    pub fn replace_before_cursor(&mut self, byte_start: usize, replacement: &str) {
        let clamped = byte_start.min(self.cursor);
        let start = if self.text.is_char_boundary(clamped) {
            clamped
        } else {
            self.text[..clamped]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i)
        };
        self.history_idx = None;
        self.text.replace_range(start..self.cursor, replacement);
        self.cursor = start + replacement.len();
        self.prune_atoms();
    }

    /// 退格删除：若光标紧邻或位于原子元素内，整块原子删除并返回被删条目。
    pub fn backspace(&mut self) -> Option<AtomicToken> {
        if self.cursor == 0 {
            return None;
        }
        self.history_idx = None;
        if let Some((atom_idx, start, end)) = self.find_atom_around_cursor(true) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
            return Some(self.atoms.remove(atom_idx));
        }
        let prev = self.prev_grapheme(self.cursor);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.prune_atoms();
        None
    }

    /// 前向删除：若光标位于原子元素起点或内部，整块原子删除并返回被删条目。
    pub fn delete(&mut self) -> Option<AtomicToken> {
        if self.cursor >= self.text.len() {
            return None;
        }
        self.history_idx = None;
        if let Some((atom_idx, start, end)) = self.find_atom_around_cursor(false) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
            return Some(self.atoms.remove(atom_idx));
        }
        let next = self.next_grapheme(self.cursor);
        self.text.replace_range(self.cursor..next, "");
        self.prune_atoms();
        None
    }

    pub fn delete_word_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.history_idx = None;
        if let Some((atom_idx, start, end)) = self.find_atom_around_cursor(true) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
            self.atoms.remove(atom_idx);
            return;
        }
        let mut p = self.cursor;
        while p > 0 {
            let prev = self.prev_grapheme(p);
            if !self.text[prev..p].trim().is_empty() {
                break;
            }
            p = prev;
        }
        while p > 0 {
            let prev = self.prev_grapheme(p);
            if self.text[prev..p].trim().is_empty() {
                break;
            }
            p = prev;
        }
        // 不得把原子块拦腰截断
        for span in self.atom_spans() {
            if p > span.start && p < span.end {
                p = span.start;
            }
        }
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
        self.prune_atoms();
    }

    pub fn delete_to_start(&mut self) {
        let mut start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        for span in self.atom_spans() {
            if start > span.start && start < span.end {
                start = span.start;
            }
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.prune_atoms();
    }

    pub fn delete_to_end(&mut self) {
        let mut end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        for span in self.atom_spans() {
            if end > span.start && end < span.end {
                end = span.end;
            }
        }
        self.text.replace_range(self.cursor..end, "");
        self.prune_atoms();
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let target = self.prev_grapheme(self.cursor);
        for span in self.atom_spans() {
            if target > span.start && target < span.end {
                self.cursor = span.start;
                return;
            }
        }
        self.cursor = target;
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let target = self.next_grapheme(self.cursor);
        for span in self.atom_spans() {
            if target > span.start && target < span.end {
                self.cursor = span.end;
                return;
            }
        }
        self.cursor = target;
    }

    pub fn move_home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |i| i + 1);
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
    }

    /// 光标移至整个输入框最开头（`Ctrl+A`）。
    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    /// 光标移至整个输入框最末尾（`Ctrl+E`）。
    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// 在多行输入框中上移一个视觉行；若已在首行则返回 `false`（可回退到历史上一条）。
    pub fn move_up(&mut self, width: u16) -> bool {
        let lines = self.visual_lines(width);
        let (cur_row, cur_col) = self.cursor_visual_in(&lines);
        if cur_row == 0 {
            return false;
        }
        let target_vl = lines[cur_row - 1];
        self.cursor = self.byte_at_visual_col(target_vl, usize::from(cur_col));
        self.snap_cursor_out_of_atom(false);
        true
    }

    /// 在多行输入框中下移一个视觉行；若已在末行则返回 `false`（可回退到历史下一条）。
    pub fn move_down(&mut self, width: u16) -> bool {
        let lines = self.visual_lines(width);
        let (cur_row, cur_col) = self.cursor_visual_in(&lines);
        if cur_row + 1 >= lines.len() {
            return false;
        }
        let target_vl = lines[cur_row + 1];
        self.cursor = self.byte_at_visual_col(target_vl, usize::from(cur_col));
        self.snap_cursor_out_of_atom(true);
        true
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                self.draft = (self.text.clone(), self.atoms.clone());
                let idx = self.history.len() - 1;
                self.history_idx = Some(idx);
                self.text = self.history[idx].0.clone();
                self.atoms = self.history[idx].1.clone();
                self.cursor = self.text.len();
            }
            Some(idx) if idx > 0 => {
                let next = idx - 1;
                self.history_idx = Some(next);
                self.text = self.history[next].0.clone();
                self.atoms = self.history[next].1.clone();
                self.cursor = self.text.len();
            }
            _ => {}
        }
    }

    pub fn history_next(&mut self) {
        let Some(idx) = self.history_idx else {
            return;
        };
        if idx + 1 < self.history.len() {
            let next = idx + 1;
            self.history_idx = Some(next);
            self.text = self.history[next].0.clone();
            self.atoms = self.history[next].1.clone();
            self.cursor = self.text.len();
        } else {
            self.history_idx = None;
            let (draft_text, draft_atoms) = std::mem::take(&mut self.draft);
            self.text = draft_text;
            self.atoms = draft_atoms;
            self.cursor = self.text.len();
        }
    }

    pub fn visual_lines(&self, width: u16) -> Vec<VisualLine> {
        let max_w = usize::from(width.max(1));
        let mut out = Vec::new();
        let mut line_start = 0;
        let mut used = 0;

        for (idx, g) in self.text.grapheme_indices(true) {
            if g == "\n" {
                out.push(VisualLine {
                    start: line_start,
                    end: idx,
                });
                line_start = idx + 1;
                used = 0;
                continue;
            }
            let w = if g == "\t" { 4 } else { g.width() };
            if used + w > max_w && idx > line_start {
                out.push(VisualLine {
                    start: line_start,
                    end: idx,
                });
                line_start = idx;
                used = w;
            } else {
                used += w;
            }
        }
        out.push(VisualLine {
            start: line_start,
            end: self.text.len(),
        });
        out
    }

    /// 返回 `(row, col)`（均从 0 开始，col 为显示列宽）。
    pub fn cursor_visual(&self, width: u16) -> (usize, u16) {
        let lines = self.visual_lines(width);
        self.cursor_visual_in(&lines)
    }

    fn cursor_visual_in(&self, lines: &[VisualLine]) -> (usize, u16) {
        for (row, vl) in lines.iter().enumerate() {
            let is_last = row + 1 == lines.len();
            let ends_with_nl = self.text.as_bytes().get(vl.end) == Some(&b'\n');
            if self.cursor >= vl.start
                && (self.cursor < vl.end
                    || (self.cursor == vl.end && (is_last || ends_with_nl)))
            {
                let col = display_width(&self.text[vl.start..self.cursor]);
                return (row, u16::try_from(col).unwrap_or(u16::MAX));
            }
        }
        let last_row = lines.len().saturating_sub(1);
        let col = lines
            .last()
            .map_or(0, |vl| display_width(&self.text[vl.start..vl.end]));
        (last_row, u16::try_from(col).unwrap_or(u16::MAX))
    }

    fn byte_at_visual_col(&self, vl: VisualLine, target_col: usize) -> usize {
        let slice = &self.text[vl.start..vl.end];
        let mut used = 0;
        for (rel, g) in slice.grapheme_indices(true) {
            let w = if g == "\t" { 4 } else { g.width() };
            if used + w > target_col {
                return vl.start + rel;
            }
            used += w;
        }
        vl.end
    }

    fn find_atom_around_cursor(&self, before: bool) -> Option<(usize, usize, usize)> {
        for (idx, atom) in self.atoms.iter().enumerate() {
            if atom.marker.is_empty() {
                continue;
            }
            for (start, _) in self.text.match_indices(&atom.marker) {
                let end = start + atom.marker.len();
                let hit = if before {
                    self.cursor > start && self.cursor <= end
                } else {
                    self.cursor >= start && self.cursor < end
                };
                if hit {
                    return Some((idx, start, end));
                }
            }
        }
        None
    }

    fn snap_cursor_out_of_atom(&mut self, prefer_end: bool) {
        for span in self.atom_spans() {
            if self.cursor > span.start && self.cursor < span.end {
                self.cursor = if prefer_end { span.end } else { span.start };
                return;
            }
        }
    }

    fn prune_atoms(&mut self) {
        self.atoms
            .retain(|a| !a.marker.is_empty() && self.text.contains(&a.marker));
    }

    fn prev_grapheme(&self, byte_idx: usize) -> usize {
        self.text[..byte_idx]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_grapheme(&self, byte_idx: usize) -> usize {
        self.text[byte_idx..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(self.text.len(), |(i, _)| byte_idx + i)
    }
}

fn display_width(s: &str) -> usize {
    if !s.contains('\t') {
        return s.width();
    }
    s.graphemes(true)
        .map(|g| if g == "\t" { 4 } else { g.width() })
        .sum()
}

fn expand_atoms(raw: &str, atoms: &[AtomicToken]) -> String {
    let mut out = raw.to_string();
    for atom in atoms {
        if atom.kind == AtomKind::PastedText
            && let Some(payload) = &atom.payload
                && out.contains(&atom.marker) {
                    out = out.replacen(&atom.marker, payload, 1);
                }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_and_large_text_delete_atomically_and_skip_cursor() {
        let mut ed = InputEditor::new();
        ed.insert_str("请看 ");
        ed.insert_image_atom("[图片 1]".into());
        assert_eq!(ed.text(), "请看 [图片 1]");

        // 左移一次跳过整个图片芯片
        ed.move_left();
        assert_eq!(ed.text_before_cursor(), "请看 ");
        // 右移一次跳回芯片末尾
        ed.move_right();
        assert_eq!(ed.text_before_cursor(), "请看 [图片 1]");

        // 一次 Backspace 整块删除图片芯片
        let removed = ed.backspace();
        assert!(removed.is_some_and(|a| a.marker == "[图片 1]"));
        assert_eq!(ed.text(), "请看 ");

        // 大段粘贴自动折叠为原子块，提交时展开为完整原文
        let big = "line1\nline2\nline3\nline4\nline5";
        let folded = ed.insert_pasted_text(big);
        assert!(folded.is_some());
        assert!(ed.text().contains("[粘贴文本 1"));

        // 左移到芯片起点，按 Delete 整块删除
        ed.move_left();
        let del = ed.delete();
        assert!(del.is_some_and(|a| a.kind == AtomKind::PastedText));
        assert_eq!(ed.text(), "请看 ");

        // 再次粘贴并提交，验证原文完整展开
        let _ = ed.insert_pasted_text(big);
        let (raw, expanded) = ed.take_and_record();
        assert!(raw.contains("[粘贴文本 2"));
        assert_eq!(expanded, format!("请看 {big}"));
    }

    #[test]
    fn multiline_up_down_navigation_works_before_history() {
        let mut ed = InputEditor::new();
        ed.insert_str("第一行\n第二行\n第三行");
        assert!(ed.move_up(20));
        assert_eq!(ed.cursor_visual(20).0, 1);
        assert!(ed.move_up(20));
        assert_eq!(ed.cursor_visual(20).0, 0);
        // 首行再按上返回 false（允许触发历史）
        assert!(!ed.move_up(20));
        assert!(ed.move_down(20));
        assert_eq!(ed.cursor_visual(20).0, 1);

        // Ctrl+A / Ctrl+E 跨多行直达整个输入框开头与末尾
        ed.move_to_start();
        assert_eq!(ed.text_before_cursor(), "");
        ed.move_to_end();
        assert_eq!(ed.text_before_cursor(), "第一行\n第二行\n第三行");
    }

    #[test]
    fn external_editor_expands_paste_and_preserves_surviving_image_atoms() {
        let mut ed = InputEditor::new();
        ed.insert_image_atom("[图片 1]".into());
        ed.insert_str(" ");
        let _ = ed.insert_pasted_text("a\nb\nc\nd\ne");
        assert_eq!(ed.expanded_text(), "[图片 1] a\nb\nc\nd\ne");

        ed.replace_from_external("已修改 [图片 1] 正文".into(), &["[图片 1]".into(), "[图片 2]".into()]);
        assert_eq!(ed.text(), "已修改 [图片 1] 正文");
        assert_eq!(ed.text_before_cursor(), "已修改 [图片 1] 正文");
        assert_eq!(ed.atom_spans().len(), 1);
        assert_eq!(ed.atom_spans()[0].kind, AtomKind::Image);
    }
}
