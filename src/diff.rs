//! 高质量 TUI Diff 视图引擎（参考 `jswysnemc/sai` 的 `render/edit_diff/` 与 `diff_cell.rs`）：
//! - 按文件后缀自动进行代码语法高亮（行号与 `+/-` 标记保持中性/增删语义色，不被高亮为数字字面量）
//! - 连续删除块与新增块按行配对，计算词边界对齐的行内精细差异强调背景（intraline emphasis）
//! - 增删行整行铺设柔和底色并保留对称左右边距；长行折行续行精准对齐到代码正文列且背景连贯无缺口
//! - 短 diff 默认完整呈现，超长 diff 自动按首 8 行 + 省略提示 + 尾 8 行折叠，点击或 `Ctrl+O` 展开全文

use std::ops::Range;
use std::path::Path;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use similar::{ChangeTag, TextDiff};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;
use crate::markdown::code::{Highlighter, find_language};
use crate::theme;

/// 折叠态保留的 diff 头部行数（与 `sai` 对齐）。
pub const DIFF_FOLD_HEAD_LINES: usize = 8;
/// 折叠态保留的 diff 尾部行数（与 `sai` 对齐）。
pub const DIFF_FOLD_TAIL_LINES: usize = 8;
/// 展开态单文件最大视觉行保护上限，避免超大生成文件卡顿终端。
const DIFF_EXPANDED_MAX_VISUAL_LINES: usize = 600;

/// 公共部分（前缀 + 后缀）占比低于该百分比时放弃行内高亮（整行重写无需行内噪声）。
const MIN_COMMON_PERCENT: usize = 25;
/// 超过该字节长度的单行跳过行内比对（如 minified 代码）。
const MAX_INTRALINE_BYTES: usize = 2048;

#[derive(Debug, Clone)]
pub enum DiffRowKind {
    HunkSep,
    Context { line_no: usize, text: String },
    Removed { line_no: usize, text: String },
    Added { line_no: usize, text: String },
}

#[derive(Debug, Clone)]
pub struct ComputedDiff {
    pub added: usize,
    pub removed: usize,
    pub rows: Vec<DiffRowKind>,
    pub max_line_no: usize,
}

impl ComputedDiff {
    /// 是否超过默认首尾折叠阈值。
    #[allow(dead_code)]
    pub fn is_foldable(&self) -> bool {
        self.rows.len() > DIFF_FOLD_HEAD_LINES + DIFF_FOLD_TAIL_LINES
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IntralinePair {
    old: Range<usize>,
    new: Range<usize>,
}

/// 基于旧文本与新文本计算带 3 行上下文分组的 Unified Diff。
pub fn compute(old_text: &str, new_text: &str) -> ComputedDiff {
    // 统一去除 \r 避免 Windows CRLF 造成整文件假 diff
    let old_norm = old_text.replace("\r\n", "\n");
    let new_norm = new_text.replace("\r\n", "\n");
    let diff = TextDiff::from_lines(&old_norm, &new_norm);

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut max_line_no = 1usize;
    let mut rows = Vec::new();

    let groups = diff.grouped_ops(3);
    for (g_idx, group) in groups.iter().enumerate() {
        if g_idx > 0 {
            rows.push(DiffRowKind::HunkSep);
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches('\n').to_string();
                match change.tag() {
                    ChangeTag::Equal => {
                        let no = change
                            .new_index()
                            .or_else(|| change.old_index())
                            .map_or(1, |i| i + 1);
                        max_line_no = max_line_no.max(no);
                        rows.push(DiffRowKind::Context { line_no: no, text });
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        let no = change.old_index().map_or(1, |i| i + 1);
                        max_line_no = max_line_no.max(no);
                        rows.push(DiffRowKind::Removed { line_no: no, text });
                    }
                    ChangeTag::Insert => {
                        added += 1;
                        let no = change.new_index().map_or(1, |i| i + 1);
                        max_line_no = max_line_no.max(no);
                        rows.push(DiffRowKind::Added { line_no: no, text });
                    }
                }
            }
        }
    }

    ComputedDiff {
        added,
        removed,
        rows,
        max_line_no,
    }
}

/// 将计算好的 diff 渲染为 `sai` 风格带语法高亮 + 行内差异强调 + 首尾折叠的 `Span` 行。
/// 返回 `(渲染行列表, 展开态因保护上限额外省略的视觉行数)`。
pub fn render_lines(
    diff: &ComputedDiff,
    path: &str,
    width: u16,
    expanded: bool,
) -> (Vec<Vec<Span<'static>>>, usize) {
    let p = theme::palette();
    // 对齐 `sai`：行号列至少 3 字符宽，右侧留 1 列对称内收
    let num_w = diff.max_line_no.to_string().len().max(3);
    // 左内收 1 列 + 行号 num_w + 1 空格 + 符号 1 列 + 2 空格间隔 + 右内收 1 列
    let left_inset = 1usize;
    let right_inset = 1usize;
    let gutter_prefix_w = num_w + 1 + 3; // "{no:>num_w$} " + "+  "
    let content_w = usize::from(width)
        .saturating_sub(left_inset + gutter_prefix_w + right_inset)
        .max(8);

    // 1. 先在全部原始行上计算相邻 Delete/Add 块的行内精细差异（Tab 展开后再算，保证字符索引严格对齐）
    let expanded_texts: Vec<String> = diff
        .rows
        .iter()
        .map(|row| match row {
            DiffRowKind::HunkSep => String::new(),
            DiffRowKind::Context { text, .. }
            | DiffRowKind::Removed { text, .. }
            | DiffRowKind::Added { text, .. } => linebreak::expand_tabs(text, 4),
        })
        .collect();
    let emphases = compute_block_emphases(&diff.rows, &expanded_texts);

    // 2. 决定保留哪些行（短 diff 完整呈现；超长未展开时首 8 + 省略提示 + 尾 8）
    let total_rows = diff.rows.len();
    let keep = DIFF_FOLD_HEAD_LINES + DIFF_FOLD_TAIL_LINES;
    let fold_middle = !expanded && total_rows > keep;

    let lang = language_from_path(Path::new(path)).and_then(find_language);
    let mut highlighter = Highlighter::new(lang);

    let mut out = Vec::new();
    let mut visual_emitted = 0usize;
    let mut overflow_omitted = 0usize;
    let visual_cap = if expanded {
        DIFF_EXPANDED_MAX_VISUAL_LINES
    } else {
        usize::MAX
    };

    let emit_row_index = |idx: usize,
                              hl: &mut Highlighter,
                              out: &mut Vec<Vec<Span<'static>>>,
                              visual_emitted: &mut usize,
                              overflow_omitted: &mut usize| {
        match &diff.rows[idx] {
            DiffRowKind::HunkSep => {
                *visual_emitted += 1;
                if *visual_emitted <= visual_cap {
                    let sep = format!(" {:>num_w$} ···", "");
                    out.push(vec![Span::styled(sep, theme::faint())]);
                } else {
                    *overflow_omitted += 1;
                }
            }
            DiffRowKind::Context { line_no, .. } => {
                let pieces = hl.line(&expanded_texts[idx]);
                emit_highlighted_diff_row(
                    *line_no,
                    "   ",
                    &pieces,
                    None,
                    None,
                    theme::faint(),
                    theme::faint(),
                    num_w,
                    content_w,
                    visual_cap,
                    visual_emitted,
                    overflow_omitted,
                    out,
                );
            }
            DiffRowKind::Removed { line_no, .. } => {
                let pieces = hl.line(&expanded_texts[idx]);
                let bg = p.diff_removed_bg;
                let emp = emphases[idx]
                    .as_ref()
                    .map(|r| (r.clone(), p.diff_removed_emphasis_bg));
                emit_highlighted_diff_row(
                    *line_no,
                    "-  ",
                    &pieces,
                    Some(bg),
                    emp,
                    Style::new().fg(p.diff_gutter_fg).bg(bg),
                    Style::new().fg(p.removed).bg(bg),
                    num_w,
                    content_w,
                    visual_cap,
                    visual_emitted,
                    overflow_omitted,
                    out,
                );
            }
            DiffRowKind::Added { line_no, .. } => {
                let pieces = hl.line(&expanded_texts[idx]);
                let bg = p.diff_added_bg;
                let emp = emphases[idx]
                    .as_ref()
                    .map(|r| (r.clone(), p.diff_added_emphasis_bg));
                emit_highlighted_diff_row(
                    *line_no,
                    "+  ",
                    &pieces,
                    Some(bg),
                    emp,
                    Style::new().fg(p.diff_gutter_fg).bg(bg),
                    Style::new().fg(p.added).bg(bg),
                    num_w,
                    content_w,
                    visual_cap,
                    visual_emitted,
                    overflow_omitted,
                    out,
                );
            }
        }
    };

    if fold_middle {
        for idx in 0..DIFF_FOLD_HEAD_LINES {
            emit_row_index(
                idx,
                &mut highlighter,
                &mut out,
                &mut visual_emitted,
                &mut overflow_omitted,
            );
        }
        let omitted_rows = total_rows - keep;
        let pad = " ".repeat(left_inset + num_w + 1);
        out.push(vec![
            Span::raw(pad),
            Span::styled(
                format!("⋮ 已省略 {omitted_rows} 行 diff（点击或 Ctrl+O 展开）"),
                theme::thought(),
            ),
        ]);
        // 跳过折叠段后重置语法高亮状态，避免多行注释跨省略段污染尾部
        let mut tail_hl = Highlighter::new(lang);
        for idx in (total_rows - DIFF_FOLD_TAIL_LINES)..total_rows {
            emit_row_index(
                idx,
                &mut tail_hl,
                &mut out,
                &mut visual_emitted,
                &mut overflow_omitted,
            );
        }
    } else {
        for idx in 0..total_rows {
            emit_row_index(
                idx,
                &mut highlighter,
                &mut out,
                &mut visual_emitted,
                &mut overflow_omitted,
            );
        }
    }

    (out, overflow_omitted)
}

#[allow(clippy::too_many_arguments)]
fn emit_highlighted_diff_row(
    line_no: usize,
    marker: &str,
    pieces: &[crate::markdown::inline::Piece],
    row_bg: Option<Color>,
    emphasis: Option<(Range<usize>, Color)>,
    gutter_style: Style,
    marker_style: Style,
    num_w: usize,
    content_w: usize,
    visual_cap: usize,
    visual_emitted: &mut usize,
    overflow_omitted: &mut usize,
    out: &mut Vec<Vec<Span<'static>>>,
) {
    // 1. 拆为字素级带样式单元，将行底色与行内强调底色叠加到语法高亮前景之上
    let mut cell_strs: Vec<String> = Vec::new();
    let mut cell_styles: Vec<Style> = Vec::new();
    let mut char_idx = 0usize;

    for piece in pieces {
        for g in piece.text.graphemes(true) {
            let g_chars = g.chars().count();
            let in_emp = emphasis.as_ref().is_some_and(|(range, _)| {
                !range.is_empty() && char_idx < range.end && (char_idx + g_chars) > range.start
            });
            let mut st = piece.style;
            if in_emp {
                if let Some((_, emp_bg)) = &emphasis {
                    st = st.bg(*emp_bg);
                }
            } else if let Some(bg) = row_bg {
                st = st.bg(bg);
            }
            cell_strs.push(g.to_string());
            cell_styles.push(st);
            char_idx += g_chars;
        }
    }

    let str_refs: Vec<&str> = cell_strs.iter().map(String::as_str).collect();
    let ranges = linebreak::lines(&str_refs, content_w);

    for (wrap_idx, range) in ranges.into_iter().enumerate() {
        *visual_emitted += 1;
        if *visual_emitted > visual_cap {
            *overflow_omitted += 1;
            continue;
        }

        let mut spans = Vec::new();
        // 左侧 1 列中性内收（对齐 `sai` 的矩形色块边距）
        spans.push(Span::raw(" "));

        if wrap_idx == 0 {
            spans.push(Span::styled(format!("{line_no:>num_w$} "), gutter_style));
            spans.push(Span::styled(marker.to_string(), marker_style));
        } else {
            // 续行精准缩进到代码正文起始列，增删行继续铺满整行底色，不留左侧缺口
            let cont_style = row_bg.map_or_else(Style::new, |bg| Style::new().bg(bg));
            spans.push(Span::styled(" ".repeat(num_w + 4), cont_style));
        }

        // 合并相邻同样式字素为连续 Span，减少 ratatui 绘制开销
        let mut used_w = 0usize;
        let mut run_text = String::new();
        let mut run_style: Option<Style> = None;
        for i in range {
            let g = &cell_strs[i];
            let st = cell_styles[i];
            used_w += g.width();
            if run_style == Some(st) {
                run_text.push_str(g);
            } else {
                if let Some(prev_st) = run_style.take()
                    && !run_text.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut run_text), prev_st));
                    }
                run_text.push_str(g);
                run_style = Some(st);
            }
        }
        if let Some(prev_st) = run_style
            && !run_text.is_empty() {
                spans.push(Span::styled(run_text, prev_st));
            }

        // 增删行右侧补齐底色至块右边界，形成规整矩形高亮条
        if let Some(bg) = row_bg {
            let pad = content_w.saturating_sub(used_w);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), Style::new().bg(bg)));
            }
        }

        out.push(spans);
    }
}

/// 收集连续删除块与紧随新增块，按行序配对计算行内变化字符区间。
fn compute_block_emphases(rows: &[DiffRowKind], expanded: &[String]) -> Vec<Option<Range<usize>>> {
    let mut emphases = vec![None; rows.len()];
    let mut i = 0usize;
    while i < rows.len() {
        if !matches!(rows[i], DiffRowKind::Removed { .. }) {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < rows.len() && matches!(rows[i], DiffRowKind::Removed { .. }) {
            i += 1;
        }
        let add_start = i;
        while i < rows.len() && matches!(rows[i], DiffRowKind::Added { .. }) {
            i += 1;
        }
        let pair_count = (add_start - del_start).min(i - add_start);
        for offset in 0..pair_count {
            let del_idx = del_start + offset;
            let add_idx = add_start + offset;
            if let Some(pair) = intraline_pair(&expanded[del_idx], &expanded[add_idx]) {
                if !pair.old.is_empty() {
                    emphases[del_idx] = Some(pair.old);
                }
                if !pair.new.is_empty() {
                    emphases[add_idx] = Some(pair.new);
                }
            }
        }
    }
    emphases
}

/// 词边界对齐的行内差异区间算法（参考 `sai/src/render/edit_diff/intraline.rs`）。
fn intraline_pair(old: &str, new: &str) -> Option<IntralinePair> {
    if old == new || old.len() > MAX_INTRALINE_BYTES || new.len() > MAX_INTRALINE_BYTES {
        return None;
    }
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    let mut prefix = 0usize;
    while prefix < old_chars.len()
        && prefix < new_chars.len()
        && old_chars[prefix] == new_chars[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < old_chars.len().saturating_sub(prefix)
        && suffix < new_chars.len().saturating_sub(prefix)
        && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let max_len = old_chars.len().max(new_chars.len());
    if max_len == 0 || (prefix + suffix) * 100 / max_len < MIN_COMMON_PERCENT {
        return None;
    }

    let prefix = retreat_to_word_start(&old_chars, prefix);
    let suffix_limit = old_chars
        .len()
        .saturating_sub(prefix)
        .min(new_chars.len().saturating_sub(prefix));
    let suffix = advance_to_word_end(&old_chars, suffix.min(suffix_limit));

    let old_range = prefix..old_chars.len().saturating_sub(suffix).max(prefix);
    let new_range = prefix..new_chars.len().saturating_sub(suffix).max(prefix);
    if old_range.is_empty() && new_range.is_empty() {
        return None;
    }
    Some(IntralinePair {
        old: old_range,
        new: new_range,
    })
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn retreat_to_word_start(chars: &[char], mut prefix: usize) -> usize {
    while prefix > 0
        && prefix < chars.len()
        && is_word_char(chars[prefix - 1])
        && is_word_char(chars[prefix])
    {
        prefix -= 1;
    }
    prefix
}

fn advance_to_word_end(chars: &[char], mut suffix: usize) -> usize {
    while suffix > 0
        && suffix < chars.len()
        && is_word_char(chars[chars.len() - suffix])
        && is_word_char(chars[chars.len() - suffix - 1])
    {
        suffix -= 1;
    }
    suffix
}

pub fn language_from_path(path: &Path) -> Option<&str> {
    let ext = path.extension()?.to_str()?;
    let mapped = match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "kt" | "kts" => "kotlin",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "sh" | "bash" | "zsh" => "bash",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "css" | "scss" | "less" => "css",
        "html" | "htm" | "xml" | "vue" | "svelte" => "html",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "go" => "go",
        "java" => "java",
        "sql" => "sql",
        "md" => "markdown",
        _ => ext,
    };
    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::{compute, intraline_pair, render_lines};
    use crate::theme::{self, ThemePreset};

    #[test]
    fn computes_hunks_and_counts_accurately() {
        theme::init(ThemePreset::MiyuLavender, true);
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n";
        let d = compute(old, new);
        assert_eq!(d.removed, 2);
        assert_eq!(d.added, 3);
        let (lines, omitted) = render_lines(&d, "sample.rs", 80, true);
        assert_eq!(omitted, 0);
        let joined: String = lines
            .iter()
            .map(|row| row.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("-  b"), "missing removed line:\n{joined}");
        assert!(joined.contains("+  B"), "missing added line:\n{joined}");
        assert!(joined.contains("+  K"), "missing added K line:\n{joined}");
    }

    #[test]
    fn intraline_highlights_only_changed_word_and_applies_emphasis_bg() {
        theme::init(ThemePreset::MiyuNight, true);
        let pair = intraline_pair("let total = old_value + 1;", "let total = new_value + 1;")
            .expect("should produce intraline pair");
        let old_seg: String = "let total = old_value + 1;"
            .chars()
            .skip(pair.old.start)
            .take(pair.old.len())
            .collect();
        let new_seg: String = "let total = new_value + 1;"
            .chars()
            .skip(pair.new.start)
            .take(pair.new.len())
            .collect();
        assert_eq!(old_seg, "old_value");
        assert_eq!(new_seg, "new_value");

        let d = compute("let total = old_value + 1;\n", "let total = new_value + 1;\n");
        let (lines, _) = render_lines(&d, "main.rs", 80, true);
        // 无论并发测试切到哪套主题，行内变化词都应与同行未变部分拥有不同的强调背景色
        let removed_bgs: std::collections::HashSet<_> = lines[0]
            .iter()
            .filter_map(|s| s.style.bg)
            .collect();
        let added_bgs: std::collections::HashSet<_> = lines[1]
            .iter()
            .filter_map(|s| s.style.bg)
            .collect();
        assert!(removed_bgs.len() >= 2, "expected distinct removed intraline emphasis bg: {removed_bgs:?}");
        assert!(added_bgs.len() >= 2, "expected distinct added intraline emphasis bg: {added_bgs:?}");
    }

    #[test]
    fn long_diff_folds_head_and_tail_until_expanded() {
        theme::init(ThemePreset::MiyuLavender, true);
        let old: String = (1..=30).map(|i| format!("let x{i} = {i};\n")).collect();
        let new: String = (1..=30).map(|i| format!("let x{i} = {};\n", i + 100)).collect();
        let d = compute(&old, &new);
        assert!(d.is_foldable());

        let (folded, _) = render_lines(&d, "big.rs", 80, false);
        let folded_txt: String = folded
            .iter()
            .map(|r| r.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(folded_txt.contains("已省略"), "expected fold notice:\n{folded_txt}");
        assert!(folded_txt.contains("Ctrl+O"));

        let (expanded, _) = render_lines(&d, "big.rs", 80, true);
        assert!(expanded.len() > folded.len());
    }
}
