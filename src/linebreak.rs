//! 中英文混排与避头尾折行算法（参考 LiteMiyu `linebreak.rs`）。
//! 英文按空格断词，中文任意字间可断，句读与括号严格避头尾，超长词/路径智能断在 `/`、`\`、`.` 等分隔符或硬切。

use std::ops::Range;
use unicode_width::UnicodeWidthStr;

const NO_START: &str = "。，、．：；！？）］｝」』】〕〉》〗〙〛”’…—～ー々ゝゞ,.;:!?)]}%";
const NO_END: &str = "（［｛「『【〔〈《〖〘〚“‘([{";

/// 将不含 `\n` 的一段字素簇按 `width` 列折成多行。空段返回 `[0..0]`。
pub fn lines(cells: &[&str], width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let mut i = 0;
    while i < cells.len() {
        let w = cells[i].width();
        if used + w > width && i > start {
            if blank(cells[i]) {
                out.push(start..i + 1);
                start = i + 1;
                used = 0;
                i += 1;
                continue;
            }
            let cut = (start + 1..=i)
                .rev()
                .find(|&j| breakable(cells, j))
                .or_else(|| {
                    // 长路径或 URL 优先在路径分隔符后断开，避免生硬劈开单词
                    (start + 1..=i)
                        .rev()
                        .find(|&j| path_sep(cells[j - 1]) && !marked(NO_START, cells[j]))
                })
                .unwrap_or_else(|| hard(cells, start, i));
            out.push(start..cut);
            start = cut;
            used = cells[start..i].iter().map(|g| g.width()).sum();
            continue;
        }
        used += w;
        i += 1;
    }
    out.push(start..cells.len());
    out
}

fn breakable(cells: &[&str], j: usize) -> bool {
    let (before, after) = (cells[j - 1], cells[j]);
    if marked(NO_START, after) || marked(NO_END, before) {
        return false;
    }
    blank(before) || before.width() > 1 || after.width() > 1
}

fn path_sep(g: &str) -> bool {
    matches!(g, "/" | "\\" | "-" | "_")
}

fn hard(cells: &[&str], start: usize, at: usize) -> usize {
    if at > start + 1 && marked(NO_START, cells[at]) {
        at - 1
    } else {
        at.max(start + 1)
    }
}

fn blank(g: &str) -> bool {
    g.trim().is_empty()
}

fn marked(set: &str, g: &str) -> bool {
    g.chars().next().is_some_and(|c| set.contains(c))
}

/// 按显示宽度截断字符串并加省略号 `…`。
pub fn clip(text: &str, max_width: u16) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let room = usize::from(max_width);
    if room == 0 {
        return String::new();
    }
    if text.width() <= room {
        return text.to_string();
    }
    if room == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let w = g.width();
        if used + w + 1 > room {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

/// 展开制表符为空格，防止终端列宽计算错位。
pub fn expand_tabs(text: &str, tab: usize) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let tab = tab.max(1);
    let mut out = String::with_capacity(text.len());
    let mut col = 0;
    for ch in text.chars() {
        if ch == '\n' {
            out.push('\n');
            col = 0;
        } else if ch == '\t' {
            let spaces = tab - (col % tab);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(ch);
            col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::lines;
    use unicode_segmentation::UnicodeSegmentation;

    fn fold(text: &str, width: usize) -> Vec<String> {
        let cells: Vec<&str> = text.graphemes(true).collect();
        lines(&cells, width)
            .into_iter()
            .map(|r| cells[r].concat())
            .collect()
    }

    #[test]
    fn mixed_cjk_and_punctuation_rules() {
        assert_eq!(fold("你好世界", 5), ["你好", "世界"]);
        let got = fold("会先睡十五秒再读文件。）", 20);
        assert!(got.iter().all(|l| !l.starts_with('。')));
        assert_eq!(fold("看这里（注释）", 8), ["看这里", "（注释）"]);
    }
}
