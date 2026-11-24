//! Unicode 宽字符安全 2D 终端画布：
//! - 严格按 `unicode-width` 维护 CJK 双宽字符占位，杜绝错位与半格残影
//! - 支持细线框线四方向位掩码自动合成（`─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`）
//! - 统一导出为带样式的 `Folded` 行序列

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::markdown::inline::Folded;

/// 细线方向位掩码：上
pub const DIR_UP: u8 = 0b0001;
/// 细线方向位掩码：下
pub const DIR_DOWN: u8 = 0b0010;
/// 细线方向位掩码：左
pub const DIR_LEFT: u8 = 0b0100;
/// 细线方向位掩码：右
pub const DIR_RIGHT: u8 = 0b1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Empty,
    Char { ch: char, style: Style },
    Continuation,
}

/// 固定宽度、按需纵深扩展的 2D 字符画布。
pub struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    box_masks: Vec<u8>,
}

impl Canvas {
    /// 创建指定宽高的空白画布。
    ///
    /// 参数:
    /// - `width`: 画布列数
    /// - `height`: 画布行数
    ///
    /// 返回:
    /// - 初始化为空白的 `Canvas`
    pub fn new(width: usize, height: usize) -> Self {
        let w = width.max(1);
        let h = height.max(1);
        Self {
            width: w,
            height: h,
            cells: vec![Cell::Empty; w * h],
            box_masks: vec![0; w * h],
        }
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(y * self.width + x)
        } else {
            None
        }
    }

    /// 清理指定单元格，若该位置属于双宽字符的左/右半格则连带清理另一半。
    fn clear_cell(&mut self, x: usize, y: usize) {
        let Some(idx) = self.index(x, y) else {
            return;
        };
        match self.cells[idx] {
            Cell::Continuation => {
                if x > 0 {
                    let prev = idx - 1;
                    self.cells[prev] = Cell::Empty;
                    self.box_masks[prev] = 0;
                }
            }
            Cell::Char { ch, .. } if ch.width().unwrap_or(1) > 1 && x + 1 < self.width => {
                self.cells[idx + 1] = Cell::Empty;
                self.box_masks[idx + 1] = 0;
            }
            _ => {}
        }
        self.cells[idx] = Cell::Empty;
        self.box_masks[idx] = 0;
    }

    /// 在指定坐标写入单个字符，自动维护双宽字符占位。
    ///
    /// 参数:
    /// - `x`: 列坐标
    /// - `y`: 行坐标
    /// - `ch`: 待写入字符
    /// - `style`: 终端前景色与修饰样式
    pub fn put_char(&mut self, x: usize, y: usize, ch: char, style: Style) {
        let w = ch.width().unwrap_or(0);
        if w == 0 || y >= self.height || x >= self.width {
            return;
        }
        if w == 2 && x + 1 >= self.width {
            return;
        }
        self.clear_cell(x, y);
        if w == 2 {
            self.clear_cell(x + 1, y);
        }
        let idx = y * self.width + x;
        self.cells[idx] = Cell::Char { ch, style };
        self.box_masks[idx] = box_char_to_mask(ch);
        if w == 2 {
            self.cells[idx + 1] = Cell::Continuation;
            self.box_masks[idx + 1] = 0;
        }
    }

    /// 在指定坐标按水平顺序写入字符串，超出右边界自动截断。
    ///
    /// 参数:
    /// - `x`: 起始列
    /// - `y`: 行坐标
    /// - `text`: 文本内容
    /// - `style`: 文本样式
    pub fn put_str(&mut self, x: usize, y: usize, text: &str, style: Style) {
        let mut col = x;
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if col + w > self.width {
                break;
            }
            self.put_char(col, y, ch, style);
            col += w;
        }
    }

    /// 在 `[x, x + span_w)` 范围内水平居中写入单行文本。
    ///
    /// 参数:
    /// - `x`: 区域左边界
    /// - `y`: 行坐标
    /// - `span_w`: 可用列宽
    /// - `text`: 文本内容
    /// - `style`: 文本样式
    pub fn put_centered(&mut self, x: usize, y: usize, span_w: usize, text: &str, style: Style) {
        let clipped = clip_width(text, span_w);
        let tw = clipped.width();
        let pad = span_w.saturating_sub(tw) / 2;
        self.put_str(x + pad, y, &clipped, style);
    }

    /// 以位掩码合并方式在坐标 `(x, y)` 叠加细线方向，交叉处自动合成连接符。
    ///
    /// 参数:
    /// - `x`: 列坐标
    /// - `y`: 行坐标
    /// - `mask`: `DIR_UP | DIR_DOWN | DIR_LEFT | DIR_RIGHT` 组合
    /// - `style`: 线条样式
    pub fn merge_line(&mut self, x: usize, y: usize, mask: u8, style: Style) {
        let Some(idx) = self.index(x, y) else {
            return;
        };
        let combined = self.box_masks[idx] | mask;
        if let Some(ch) = mask_to_box_char(combined) {
            self.clear_cell(x, y);
            self.cells[idx] = Cell::Char { ch, style };
            self.box_masks[idx] = combined;
        }
    }

    /// 绘制从 `x1` 到 `x2`（含端点）的水平线段。
    ///
    /// 参数:
    /// - `y`: 行坐标
    /// - `x1`: 起点列
    /// - `x2`: 终点列
    /// - `dashed`: 为 `true` 时使用虚线 `┄`
    /// - `style`: 线条样式
    pub fn draw_hline(&mut self, y: usize, x1: usize, x2: usize, dashed: bool, style: Style) {
        if y >= self.height {
            return;
        }
        let (start, end) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
        let end = end.min(self.width.saturating_sub(1));
        for x in start..=end {
            if dashed {
                self.put_char(x, y, '┄', style);
            } else if start == end {
                self.merge_line(x, y, DIR_LEFT | DIR_RIGHT, style);
            } else {
                let mut mask = 0u8;
                if x > start {
                    mask |= DIR_LEFT;
                }
                if x < end {
                    mask |= DIR_RIGHT;
                }
                self.merge_line(x, y, mask, style);
            }
        }
    }

    /// 绘制从 `y1` 到 `y2`（含端点）的垂直线段。
    ///
    /// 参数:
    /// - `x`: 列坐标
    /// - `y1`: 起点行
    /// - `y2`: 终点行
    /// - `dashed`: 为 `true` 时使用虚线 `┆`
    /// - `style`: 线条样式
    pub fn draw_vline(&mut self, x: usize, y1: usize, y2: usize, dashed: bool, style: Style) {
        if x >= self.width {
            return;
        }
        let (start, end) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
        let end = end.min(self.height.saturating_sub(1));
        for y in start..=end {
            if dashed {
                self.put_char(x, y, '┆', style);
            } else if start == end {
                self.merge_line(x, y, DIR_UP | DIR_DOWN, style);
            } else {
                let mut mask = 0u8;
                if y > start {
                    mask |= DIR_UP;
                }
                if y < end {
                    mask |= DIR_DOWN;
                }
                self.merge_line(x, y, mask, style);
            }
        }
    }

    /// 绘制矩形或圆角方框。
    ///
    /// 参数:
    /// - `x`: 左上角列坐标
    /// - `y`: 左上角行坐标
    /// - `w`: 含边框总宽（至少 2）
    /// - `h`: 含边框总高（至少 2）
    /// - `rounded`: 为 `true` 时使用圆角 `╭╮╰╯`，否则使用直角 `┌┐└┘`
    /// - `style`: 边框样式
    pub fn draw_box(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        rounded: bool,
        style: Style,
    ) {
        if w < 2 || h < 2 {
            return;
        }
        let right = x + w - 1;
        let bottom = y + h - 1;
        let (tl, tr, bl, br) = if rounded {
            ('╭', '╮', '╰', '╯')
        } else {
            ('┌', '┐', '└', '┘')
        };
        self.put_char(x, y, tl, style);
        self.put_char(right, y, tr, style);
        self.put_char(x, bottom, bl, style);
        self.put_char(right, bottom, br, style);
        for col in (x + 1)..right {
            self.put_char(col, y, '─', style);
            self.put_char(col, bottom, '─', style);
        }
        for row in (y + 1)..bottom {
            self.put_char(x, row, '│', style);
            self.put_char(right, row, '│', style);
        }
    }

    /// 将画布裁剪掉底部全空行并转换为 `Vec<Folded>`，相邻同样式字符自动合并。
    ///
    /// 返回:
    /// - 可直接写入 `MdLine` 的行集合
    pub fn into_folded(self) -> Vec<Folded> {
        let mut last_non_empty = 0usize;
        for y in 0..self.height {
            let row_slice = &self.cells[y * self.width..(y + 1) * self.width];
            if row_slice.iter().any(|c| !matches!(c, Cell::Empty)) {
                last_non_empty = y + 1;
            }
        }
        let mut out = Vec::with_capacity(last_non_empty);
        for y in 0..last_non_empty {
            let row_slice = &self.cells[y * self.width..(y + 1) * self.width];
            let mut end_col = self.width;
            while end_col > 0 && matches!(row_slice[end_col - 1], Cell::Empty) {
                end_col -= 1;
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut cur_style: Option<Style> = None;
            let mut cur_text = String::new();

            for cell in &row_slice[..end_col] {
                match *cell {
                    Cell::Continuation => continue,
                    Cell::Empty => {
                        let st = Style::new();
                        if cur_style == Some(st) {
                            cur_text.push(' ');
                        } else {
                            if !cur_text.is_empty() {
                                spans.push(Span::styled(
                                    std::mem::take(&mut cur_text),
                                    cur_style.unwrap_or_default(),
                                ));
                            }
                            cur_style = Some(st);
                            cur_text.push(' ');
                        }
                    }
                    Cell::Char { ch, style } => {
                        if cur_style == Some(style) {
                            cur_text.push(ch);
                        } else {
                            if !cur_text.is_empty() {
                                spans.push(Span::styled(
                                    std::mem::take(&mut cur_text),
                                    cur_style.unwrap_or_default(),
                                ));
                            }
                            cur_style = Some(style);
                            cur_text.push(ch);
                        }
                    }
                }
            }
            if !cur_text.is_empty() {
                spans.push(Span::styled(cur_text, cur_style.unwrap_or_default()));
            }
            out.push(Folded {
                spans,
                links: Vec::new(),
                joined: false,
            });
        }
        out
    }
}

/// 按显示列宽截断文本，超出时以 `…` 收尾。
///
/// 参数:
/// - `text`: 原始文本
/// - `max_width`: 最大显示列宽
///
/// 返回:
/// - 不超过 `max_width` 列的字符串
pub fn clip_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    let limit = max_width - 1;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > limit {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

fn box_char_to_mask(ch: char) -> u8 {
    match ch {
        '─' => DIR_LEFT | DIR_RIGHT,
        '│' => DIR_UP | DIR_DOWN,
        '┌' | '╭' => DIR_DOWN | DIR_RIGHT,
        '┐' | '╮' => DIR_DOWN | DIR_LEFT,
        '└' | '╰' => DIR_UP | DIR_RIGHT,
        '┘' | '╯' => DIR_UP | DIR_LEFT,
        '├' => DIR_UP | DIR_DOWN | DIR_RIGHT,
        '┤' => DIR_UP | DIR_DOWN | DIR_LEFT,
        '┬' => DIR_DOWN | DIR_LEFT | DIR_RIGHT,
        '┴' => DIR_UP | DIR_LEFT | DIR_RIGHT,
        '┼' => DIR_UP | DIR_DOWN | DIR_LEFT | DIR_RIGHT,
        _ => 0,
    }
}

fn mask_to_box_char(mask: u8) -> Option<char> {
    let ch = match mask {
        0 => return None,
        m if m == DIR_UP || m == DIR_DOWN || m == (DIR_UP | DIR_DOWN) => '│',
        m if m == DIR_LEFT || m == DIR_RIGHT || m == (DIR_LEFT | DIR_RIGHT) => '─',
        m if m == (DIR_DOWN | DIR_RIGHT) => '┌',
        m if m == (DIR_DOWN | DIR_LEFT) => '┐',
        m if m == (DIR_UP | DIR_RIGHT) => '└',
        m if m == (DIR_UP | DIR_LEFT) => '┘',
        m if m == (DIR_UP | DIR_DOWN | DIR_RIGHT) => '├',
        m if m == (DIR_UP | DIR_DOWN | DIR_LEFT) => '┤',
        m if m == (DIR_DOWN | DIR_LEFT | DIR_RIGHT) => '┬',
        m if m == (DIR_UP | DIR_LEFT | DIR_RIGHT) => '┴',
        m if m == (DIR_UP | DIR_DOWN | DIR_LEFT | DIR_RIGHT) => '┼',
        _ => return None,
    };
    Some(ch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::inline::plain;

    #[test]
    fn canvas_merges_intersecting_lines_and_keeps_cjk_alignment() {
        let mut c = Canvas::new(16, 5);
        c.draw_box(0, 0, 12, 3, true, Style::new());
        c.put_centered(1, 1, 10, "用户输入", Style::new());
        c.draw_hline(4, 2, 8, false, Style::new());
        c.draw_vline(5, 3, 4, false, Style::new());
        let rows: Vec<String> = c.into_folded().iter().map(plain).collect();
        assert_eq!(rows[0], "╭──────────╮");
        assert_eq!(rows[1], "│ 用户输入 │");
        assert_eq!(rows[2], "╰──────────╯");
        assert_eq!(rows[4].chars().nth(5), Some('┴'));
    }
}
