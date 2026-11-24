//! 表格渲染器（参考 LiteMiyu `markdown/table.rs`）：
//! 完整实线方框（`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`），每两行间有柔和分隔线；
//! 窄屏或超多列放不下时自动退化为优雅的「表头：值」竖排段落卡片。

use pulldown_cmark::Alignment;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::inline::{Folded, Piece, fold, plain};
use crate::theme;

pub type Cell = Vec<Piece>;

const RULE_MAX: usize = 20;

pub fn render(head: &[Cell], rows: &[Vec<Cell>], aligns: &[Alignment], width: u16) -> Vec<Folded> {
    let columns = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return Vec::new();
    }
    let natural: Vec<usize> = (0..columns)
        .map(|c| {
            std::iter::once(head)
                .chain(rows.iter().map(Vec::as_slice))
                .filter_map(|row| row.get(c))
                .map(cell_width)
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();
    let widths = bounded(natural, usize::from(width));
    if widths.iter().sum::<usize>() + widths.len() * 3 + 1 > usize::from(width) {
        return stacked(head, rows, width);
    }

    let rule = |left: &str, mid: &str, right: &str| {
        let body: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        line(vec![Span::styled(
            format!("{left}{}{right}", body.join(mid)),
            theme::faint(),
        )])
    };

    let mut out = vec![rule("┌", "┬", "┐")];
    out.extend(row_lines(head, &widths, aligns, true));
    for row in rows {
        out.push(rule("├", "┼", "┤"));
        out.extend(row_lines(row, &widths, aligns, false));
    }
    out.push(rule("└", "┴", "┘"));
    out
}

fn bounded(mut widths: Vec<usize>, room: usize) -> Vec<usize> {
    let frame = widths.len() * 3 + 1;
    let floor = match widths.len() {
        1 => 16,
        2 => 14,
        3 | 4 => 10,
        _ => 8,
    };
    while widths.iter().sum::<usize>() + frame > room {
        let Some((i, w)) = widths.iter().copied().enumerate().max_by_key(|(_, w)| *w) else {
            break;
        };
        if w <= floor {
            break;
        }
        widths[i] -= 1;
    }
    widths
}

fn stacked(head: &[Cell], rows: &[Vec<Cell>], width: u16) -> Vec<Folded> {
    let rule = "─".repeat(usize::from(width).min(RULE_MAX));
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        if r > 0 {
            out.push(line(vec![Span::styled(rule.clone(), theme::faint())]));
        }
        for (c, cell) in row.iter().enumerate() {
            let title: String = head
                .get(c)
                .map(|h| h.iter().map(|p| p.text.as_str()).collect())
                .unwrap_or_default();
            let title = title.trim();
            let mut pieces = Vec::new();
            if !title.is_empty() {
                let wide = title
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.width().unwrap_or(1) > 1);
                let colon = if wide { "：" } else { ": " };
                pieces.push(Piece::new(format!("{title}{colon}"), theme::md_table_head()));
            }
            pieces.extend(cell.iter().cloned());
            out.extend(fold(&pieces, width.max(1)));
        }
    }
    out
}

fn cell_width(cell: &Cell) -> usize {
    let text: String = cell.iter().map(|p| p.text.as_str()).collect();
    text.split('\n')
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

fn row_lines(row: &[Cell], widths: &[usize], aligns: &[Alignment], head: bool) -> Vec<Folded> {
    let empty = Cell::new();
    let cells: Vec<Vec<Folded>> = widths
        .iter()
        .enumerate()
        .map(|(c, w)| {
            let pieces: Vec<Piece> = row
                .get(c)
                .unwrap_or(&empty)
                .iter()
                .map(|p| {
                    if head {
                        Piece {
                            style: p.style.patch(theme::md_table_head()),
                            ..p.clone()
                        }
                    } else {
                        p.clone()
                    }
                })
                .collect();
            let mut folded = fold(&pieces, u16::try_from(*w).unwrap_or(u16::MAX));
            folded.iter_mut().for_each(|f| fit(f, *w));
            folded
        })
        .collect();

    let height = cells.iter().map(Vec::len).max().unwrap_or(1);
    (0..height)
        .map(|r| {
            let mut spans = Vec::new();
            let mut links = Vec::new();
            let mut col = 0u16;
            for (c, w) in widths.iter().enumerate() {
                let bar = if c == 0 { "│ " } else { " │ " };
                spans.push(Span::styled(bar, theme::faint()));
                col += u16::try_from(bar.width()).unwrap_or(0);
                let folded = cells[c].get(r);
                let used = folded.map_or(0, |f| plain(f).width());
                let gap = w.saturating_sub(used);
                let (before, after) = match aligns.get(c) {
                    Some(Alignment::Right) => (gap, 0),
                    Some(Alignment::Center) => (gap / 2, gap - gap / 2),
                    _ => (0, gap),
                };
                if before > 0 {
                    spans.push(Span::raw(" ".repeat(before)));
                    col += u16::try_from(before).unwrap_or(0);
                }
                if let Some(f) = folded {
                    links.extend(
                        f.links
                            .iter()
                            .map(|(a, b, u)| (a + col, b + col, u.clone())),
                    );
                    spans.extend(f.spans.iter().cloned());
                }
                if after > 0 {
                    spans.push(Span::raw(" ".repeat(after)));
                }
                col += u16::try_from(used + after).unwrap_or(0);
            }
            spans.push(Span::styled(" │", theme::faint()));
            Folded {
                spans,
                links,
                joined: false,
            }
        })
        .collect()
}

fn fit(folded: &mut Folded, width: usize) {
    let mut over = plain(folded).width().saturating_sub(width);
    while over > 0 {
        let Some(last) = folded.spans.last_mut() else {
            return;
        };
        let mut text = last.content.to_string();
        while over > 0 {
            let Some(c) = text.chars().next_back().filter(|c| c.is_whitespace()) else {
                break;
            };
            text.pop();
            over = over.saturating_sub(c.width().unwrap_or(0));
        }
        let emptied = text.is_empty();
        last.content = text.into();
        if emptied {
            folded.spans.pop();
        } else if over > 0 {
            return;
        }
    }
}

fn line(spans: Vec<Span<'static>>) -> Folded {
    Folded {
        spans,
        links: Vec::new(),
        joined: false,
    }
}

#[cfg(test)]
mod tests {
    use pulldown_cmark::Alignment;
    use ratatui::style::Style;
    use unicode_width::UnicodeWidthStr;

    use super::render;
    use crate::markdown::inline::{Piece, plain};

    fn cell(text: &str) -> Vec<Piece> {
        vec![Piece::new(text, Style::new())]
    }

    #[test]
    fn table_has_perfectly_aligned_box_borders() {
        let head = vec![cell("字段"), cell("说明")];
        let rows = vec![
            vec![cell("方案属性"), cell("从源头补齐 OFF -> LOCKSCREEN 终态输出")],
            vec![cell("影响范围"), cell("LSSI/Malibu 共性修复")],
        ];
        let lines: Vec<String> = render(&head, &rows, &[Alignment::Left, Alignment::Left], 48)
            .iter()
            .map(plain)
            .collect();
        assert!(lines[0].starts_with('┌'));
        assert!(lines.last().unwrap().starts_with('└'));
        assert!(lines.iter().all(|l| l.width() == lines[0].width()));
    }
}
