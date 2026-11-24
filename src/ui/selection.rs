//! 终端划词选区、扁平单块屏幕文本快照与所见即所得选区复制提取。
//! 相比逐格克隆 `Vec<Vec<(u16, String)>>`（每帧数千次堆分配），本模块将整屏可见格压平到
//! 单一连续 `String` 与扁平索引切片中，在任意时刻零竞态支持划词提取的同时消除高频堆分配。

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::ui::rows::RowCopyMeta;

#[derive(Debug, Clone, Copy)]
pub struct TextSelection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// 限制选区的列边界（避免在正文区拖选时误框入右侧边栏或滚动条）
    pub zone: Rect,
}

impl TextSelection {
    /// 返回规范化后的 `(start, end)`，按 `(row, col)` 从小到大排序。
    ///
    /// 返回:
    /// - 排序后的起止坐标元组 `((start_col, start_row), (end_col, end_row))`
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let a = (self.anchor.1, self.anchor.0);
        let b = (self.head.1, self.head.0);
        if a <= b {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// 计算指定屏幕行在选区与限定区域内的有效列区间。
    ///
    /// 参数:
    /// - `y`: 屏幕行号
    ///
    /// 返回:
    /// - 命中时返回 `Some((from_col, to_col))`，否则返回 `None`
    pub fn row_col_range(&self, y: u16) -> Option<(u16, u16)> {
        let ((sx, sy), (ex, ey)) = self.ordered();
        if y < sy
            || y > ey
            || self.zone.width == 0
            || self.zone.height == 0
            || y < self.zone.y
            || y >= self.zone.bottom()
        {
            return None;
        }
        let z_left = self.zone.x;
        let z_right = self.zone.right().saturating_sub(1);
        let from = if y == sy { sx.clamp(z_left, z_right) } else { z_left };
        let to = if y == ey { ex.clamp(z_left, z_right) } else { z_right };
        (from <= to).then_some((from, to))
    }
}

/// 扁平单块屏幕文本快照：整屏仅分配 3 块连续内存，替代逐单元格 `String` 分配。
#[derive(Default, Debug, Clone)]
pub struct ScreenSnapshot {
    pub width: u16,
    pub height: u16,
    text: String,
    /// `(col, display_width, byte_start, byte_end)`
    cells: Vec<(u16, u16, u32, u32)>,
    /// 每行在 `cells` 中的半开区间 `(start, end)`
    row_ranges: Vec<(usize, usize)>,
}

impl ScreenSnapshot {
    /// 从当前帧缓冲区捕获扁平纯文本快照（跳过宽字符第二列占位格）。
    ///
    /// 参数:
    /// - `buf`: ratatui 当前帧缓冲区
    /// - `area`: 捕获矩形区域
    ///
    /// 返回:
    /// - 紧凑存储的 `ScreenSnapshot`
    pub fn capture(buf: &Buffer, area: Rect) -> Self {
        let rows_count = usize::from(area.bottom());
        let est_cells = usize::from(area.width).saturating_mul(usize::from(area.height));
        let mut text = String::with_capacity(est_cells);
        let mut cells = Vec::with_capacity(est_cells);
        let mut row_ranges = Vec::with_capacity(rows_count);

        for y in 0..area.bottom() {
            let start = cells.len();
            if y >= area.y {
                let mut skip_until = 0u16;
                for x in area.x..area.right() {
                    if x < skip_until {
                        continue;
                    }
                    let sym = buf[(x, y)].symbol();
                    let w = u16::try_from(sym.width()).unwrap_or(1).max(1);
                    let byte_start = u32::try_from(text.len()).unwrap_or(u32::MAX);
                    text.push_str(sym);
                    let byte_end = u32::try_from(text.len()).unwrap_or(u32::MAX);
                    cells.push((x, w, byte_start, byte_end));
                    skip_until = x.saturating_add(w);
                }
            }
            row_ranges.push((start, cells.len()));
        }

        Self {
            width: area.width,
            height: area.bottom(),
            text,
            cells,
            row_ranges,
        }
    }

    /// 迭代指定屏幕行的可见单元 `(col, width, symbol)`。
    ///
    /// 参数:
    /// - `y`: 屏幕行号
    ///
    /// 返回:
    /// - 该行单元迭代器（行越界时为空）
    pub fn row_cells(&self, y: u16) -> impl Iterator<Item = (u16, u16, &str)> {
        let range = self
            .row_ranges
            .get(usize::from(y))
            .copied()
            .unwrap_or((0, 0));
        self.cells[range.0..range.1]
            .iter()
            .map(|&(x, w, s, e)| (x, w, &self.text[s as usize..e as usize]))
    }

    /// 拼接指定屏幕行在列区间 `[col_start, col_end)` 内的原始文本。
    ///
    /// 参数:
    /// - `y`: 屏幕行号
    /// - `col_start`: 起始列（含）
    /// - `col_end`: 结束列（不含）
    ///
    /// 返回:
    /// - 拼接后的子串
    #[cfg(test)]
    pub fn slice_row_text(&self, y: u16, col_start: u16, col_end: u16) -> String {
        let mut out = String::new();
        for (x, _, sym) in self.row_cells(y) {
            if x >= col_start && x < col_end {
                out.push_str(sym);
            }
        }
        out
    }
}

/// 从屏幕快照中提取划词选区文字（跳过装饰列、滚动条与边栏竖线，自动拼接软折行）。
///
/// 参数:
/// - `snapshot`: 本帧扁平屏幕快照
/// - `sel`: 划词选区范围
/// - `body_text`: 正文纯文字区域
/// - `scrollbar_col`: 正文右侧滚动条所在列
/// - `sidebar_divider_col`: 侧边栏分隔竖线所在列
/// - `row_copy_meta`: 正文各行复制元信息
///
/// 返回:
/// - 规范化后的复制文本
pub fn extract_selection(
    snapshot: &ScreenSnapshot,
    sel: TextSelection,
    body_text: Rect,
    scrollbar_col: Option<u16>,
    sidebar_divider_col: Option<u16>,
    row_copy_meta: &HashMap<u16, RowCopyMeta>,
) -> String {
    let ((_, sy), (_, ey)) = sel.ordered();
    let multi_row = sy < ey;
    let in_body = body_text.width > 0 && sel.zone == body_text;
    let mut lines: Vec<String> = Vec::new();
    let mut prev_emitted_y: Option<u16> = None;

    for y in sy..=ey {
        let Some((raw_from, to)) = sel.row_col_range(y) else {
            continue;
        };
        let meta = if in_body {
            row_copy_meta.get(&y).copied()
        } else {
            None
        };
        // 1. 多行框选时自动跳过代码块语言标签、空竖线等纯装饰行
        if multi_row && meta.is_some_and(|m| m.decorative) {
            continue;
        }
        let from = if let Some(m) = meta {
            raw_from.max(body_text.x.saturating_add(m.content_left))
        } else {
            raw_from
        };
        if from > to {
            continue;
        }
        let mut line = String::new();
        for (x, w, sym) in snapshot.row_cells(y) {
            if x + w > from && x <= to {
                // 2. 兜底过滤正文右侧滚动条（┃/│）与侧边栏分隔竖线（│）
                let is_right_chrome = (scrollbar_col == Some(x) && matches!(sym, "┃" | "│"))
                    || (sidebar_divider_col == Some(x) && sym == "│");
                if !is_right_chrome {
                    line.push_str(sym);
                }
            }
        }
        let trimmed = line.trim_end().to_string();
        if meta.is_some_and(|m| m.join_previous)
            && prev_emitted_y == Some(y.saturating_sub(1))
            && let Some(last) = lines.last_mut()
        {
            last.push_str(&trimmed);
        } else {
            lines.push(trimmed);
        }
        prev_emitted_y = Some(y);
    }
    // 3. 去掉首尾全空行（保留中间有意义的空行）
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// 在渲染缓冲区上为划词选区叠加反色高亮（跳过纯装饰行与左侧装饰列）。
///
/// 参数:
/// - `buf`: ratatui 渲染缓冲区
/// - `full`: 终端全屏区域
/// - `sel`: 当前选区
/// - `body_text`: 正文纯文字区域
/// - `row_copy_meta`: 正文各行复制元信息
pub fn apply_selection_highlight(
    buf: &mut Buffer,
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
                let rect = Rect::new(from, y, to.saturating_sub(from) + 1, 1).intersection(full);
                buf.set_style(rect, Style::new().add_modifier(Modifier::REVERSED));
            }
        }
    }
}
