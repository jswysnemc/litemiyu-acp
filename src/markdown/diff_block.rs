//! Markdown `diff` / `patch` 代码块精细着色器：支持增删整行柔和底色、文件头路径分层、Hunk 拆分与自动源码语法高亮。

use std::path::Path;

use ratatui::style::{Color, Modifier};

use super::code::{self, Highlighter};
use super::inline::Piece;
use super::path_style;
use crate::{diff, theme};

/// 判断代码块语言标识是否属于 Diff / Patch 类型。
pub fn is_diff_lang(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "diff" | "patch" | "udiff"
    )
}

/// 逐行解析并高亮 `diff` / `patch` 代码块（根据 `+++`/`---`/`diff --git` 自动推断源码语言）。
pub struct DiffBlockHighlighter {
    code_hl: Option<Highlighter>,
}

impl DiffBlockHighlighter {
    /// 创建新的 Diff 代码块高亮器。
    pub fn new() -> Self {
        Self { code_hl: None }
    }

    /// 【代码块】【Diff 高亮】高亮单行 diff 内容，返回 `(行内样式片段, 可选整行底色)`。
    ///
    /// 参数:
    /// - `line`: 原始 diff 单行文本
    ///
    /// 返回:
    /// - `(着色片段列表, 增删行对应的柔和底色)`；普通行底色为 `None`（沿用代码块默认底色）
    pub fn line(&mut self, line: &str) -> (Vec<Piece>, Option<Color>) {
        if line.is_empty() {
            return (Vec::new(), None);
        }
        let p = theme::palette();

        // 1. `diff --git a/... b/...` 文件分隔头：提取右侧路径推断语言，并对两段路径分层着色
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let mut pieces = vec![
                Piece::new("diff --git", theme::code_keyword()),
                Piece::new(" ", theme::text()),
            ];
            let mut tokens = rest.splitn(2, ' ');
            if let Some(left) = tokens.next() {
                self.update_lang_from_diff_path(left);
                pieces.extend(style_diff_header_path(left));
            }
            if let Some(right) = tokens.next() {
                pieces.push(Piece::new(" ", theme::text()));
                self.update_lang_from_diff_path(right);
                pieces.extend(style_diff_header_path(right));
            }
            return (pieces, None);
        }

        // 2. `--- a/...` / `+++ b/...` 文件头：推断目标语言并对路径分层着色
        if let Some(rest) = line.strip_prefix("--- ") {
            self.update_lang_from_diff_path(rest);
            let mut pieces = vec![Piece::new("--- ", theme::removed().add_modifier(Modifier::BOLD))];
            pieces.extend(style_diff_header_path(rest));
            return (pieces, None);
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            self.update_lang_from_diff_path(rest);
            let mut pieces = vec![Piece::new("+++ ", theme::added().add_modifier(Modifier::BOLD))];
            pieces.extend(style_diff_header_path(rest));
            return (pieces, None);
        }

        // 3. `@@ -10,4 +10,6 @@ fn context()` Hunk 范围与函数上下文拆分着色
        if line.starts_with("@@") {
            if let Some(end_rel) = line[2..].find("@@") {
                let split = 2 + end_rel + 2;
                let mut pieces = vec![Piece::new(&line[..split], theme::code_macro())];
                if split < line.len() {
                    pieces.push(Piece::new(&line[split..], theme::dim()));
                }
                return (pieces, None);
            }
            return (vec![Piece::new(line, theme::code_macro())], None);
        }

        // 4. Git 元信息行保持柔和可读（使用 dim 避免透明背景下发虚隐身）
        if let Some(rest) = line.strip_prefix("rename from ") {
            let mut pieces = vec![Piece::new("rename from ", theme::dim())];
            pieces.extend(path_style::style_path_pieces(rest, None));
            return (pieces, None);
        }
        if let Some(rest) = line.strip_prefix("rename to ") {
            let mut pieces = vec![Piece::new("rename to ", theme::dim())];
            pieces.extend(path_style::style_path_pieces(rest, None));
            return (pieces, None);
        }
        if line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("Binary files ")
            || line.starts_with("\\ No newline at end of file")
        {
            return (vec![Piece::new(line, theme::dim())], None);
        }

        // 5. 新增行 `+...`：柔和绿底 + 加粗 `+` 标记 + 源码语法高亮（并提亮暗注释）
        if let Some(body) = line.strip_prefix('+') {
            let mut pieces = vec![Piece::new("+", theme::added().add_modifier(Modifier::BOLD))];
            if let Some(hl) = &mut self.code_hl {
                pieces.extend(lift_comment_contrast(hl.line(body)));
            } else {
                pieces.push(Piece::new(body, theme::added()));
            }
            return (pieces, Some(p.diff_added_bg));
        }

        // 6. 删除行 `-...`：柔和红底 + 加粗 `-` 标记 + 源码语法高亮（并提亮暗注释）
        if let Some(body) = line.strip_prefix('-') {
            let mut pieces = vec![Piece::new("-", theme::removed().add_modifier(Modifier::BOLD))];
            if let Some(hl) = &mut self.code_hl {
                pieces.extend(lift_comment_contrast(hl.line(body)));
            } else {
                pieces.push(Piece::new(body, theme::removed()));
            }
            return (pieces, Some(p.diff_removed_bg));
        }

        // 7. 上下文行（前导空格或普通行）：保留源码语法高亮或默认正文色
        if let Some(body) = line.strip_prefix(' ') {
            let mut pieces = vec![Piece::new(" ", theme::dim())];
            if let Some(hl) = &mut self.code_hl {
                pieces.extend(hl.line(body));
            } else {
                pieces.push(Piece::new(body, theme::dim()));
            }
            return (pieces, None);
        }

        (vec![Piece::new(line, theme::text())], None)
    }

    fn update_lang_from_diff_path(&mut self, raw: &str) {
        let trimmed = raw.trim().split('\t').next().unwrap_or("").trim();
        if trimmed.is_empty() || trimmed == "/dev/null" {
            return;
        }
        let clean = trimmed
            .strip_prefix("a/")
            .or_else(|| trimmed.strip_prefix("b/"))
            .unwrap_or(trimmed);
        if let Some(lang) = diff::language_from_path(Path::new(clean)).and_then(code::find_language)
        {
            self.code_hl = Some(Highlighter::new(Some(lang)));
        }
    }
}

/// 提亮增删底色行内的低对比度注释文字，避免暗色注释叠在绿/红底色上发黑难辨。
fn lift_comment_contrast(pieces: Vec<Piece>) -> Vec<Piece> {
    let comment_fg = theme::code_comment().fg;
    let lifted = theme::dim().add_modifier(Modifier::ITALIC);
    pieces
        .into_iter()
        .map(|mut p| {
            if p.style.fg == comment_fg {
                p.style = lifted;
            }
            p
        })
        .collect()
}

fn style_diff_header_path(raw: &str) -> Vec<Piece> {
    let trimmed = raw.trim();
    if trimmed == "/dev/null" {
        return vec![Piece::new(raw, theme::dim())];
    }
    if path_style::looks_like_file_path(trimmed) {
        path_style::style_path_pieces(raw, None)
    } else {
        vec![Piece::new(raw, theme::md_url())]
    }
}

#[cfg(test)]
mod tests {
    use crate::markdown::render;
    use crate::theme::{self, ThemePreset};

    #[test]
    fn diff_code_block_applies_row_backgrounds_hunk_split_and_syntax_highlight() {
        theme::init(ThemePreset::MiyuLavender, true);
        let p = theme::palette();
        let md = "```diff\n\
diff --git a/src/app/mouse.rs b/src/app/mouse.rs\n\
--- a/src/app/mouse.rs\n\
+++ b/src/app/mouse.rs\n\
@@ -10,3 +10,4 @@ fn handle_mouse()\n\
 let keep = true;\n\
-let old_val = 1;\n\
+// 新增注释\n\
+let new_val = 2;\n\
```\n";
        let lines = render(md, 90);

        let hunk_line = lines
            .iter()
            .find(|l| l.to_spans().iter().any(|s| s.content.contains("@@ -10,3")))
            .expect("hunk header line");
        assert!(
            hunk_line
                .to_spans()
                .iter()
                .any(|s| s.content.contains("fn handle_mouse()") && s.style.fg == theme::dim().fg),
            "hunk context suffix should be dimmed separately"
        );

        let removed_line = lines
            .iter()
            .find(|l| l.to_spans().iter().any(|s| s.content.contains("old_val")))
            .expect("removed diff line");
        assert!(
            removed_line
                .to_spans()
                .iter()
                .any(|s| s.style.bg == Some(p.diff_removed_bg)),
            "removed line should carry diff_removed_bg"
        );
        assert!(
            removed_line
                .to_spans()
                .iter()
                .any(|s| s.content.contains("let") && s.style.fg == theme::code_keyword().fg),
            "removed Rust line should retain syntax keyword color"
        );

        // 绿底上的注释应自动提亮至 dim，不再使用低对比度 code_comment
        let added_comment = lines
            .iter()
            .find(|l| l.to_spans().iter().any(|s| s.content.contains("新增注释")))
            .expect("added comment line");
        assert!(
            added_comment
                .to_spans()
                .iter()
                .any(|s| s.content.contains("新增注释") && s.style.fg == theme::dim().fg),
            "comment on diff_added_bg should lift contrast to dim"
        );

        let added_line = lines
            .iter()
            .find(|l| l.to_spans().iter().any(|s| s.content.contains("new_val")))
            .expect("added diff line");
        assert!(
            added_line
                .to_spans()
                .iter()
                .any(|s| s.style.bg == Some(p.diff_added_bg)),
            "added line should carry diff_added_bg"
        );
        assert!(
            added_line
                .to_spans()
                .iter()
                .any(|s| s.content.contains('2') && s.style.fg == theme::code_number().fg),
            "added Rust line should retain syntax number color"
        );
    }
}
