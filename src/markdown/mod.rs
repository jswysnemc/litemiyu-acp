//! Markdown 渲染主入口（参考 LiteMiyu `markdown/mod.rs`）。
//! 支持标题、段落、完整框线表格（超宽自动变卡片）、多语言语法高亮代码块、列表/任务列表、引用、
//! 粗体/斜体/删除线/行内代码、数学公式、裸链接与优雅短链接展示。

pub mod code;
pub mod dollars;
pub mod inline;
pub mod table;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;
use crate::theme;
use inline::{Folded, Piece, fold};
use table::Cell;

#[derive(Debug, Clone)]
pub struct MdLine {
    pub lead: Vec<Span<'static>>,
    pub folded: Folded,
    /// 本行正文起始列偏移（相对渲染起点，供划词复制跳过代码块边距等装饰列）
    pub content_offset: u16,
    /// 是否为纯装饰行（如代码块语言标签），多行划词复制时自动忽略
    pub decorative: bool,
}

impl MdLine {
    fn plain(lead: Vec<Span<'static>>, folded: Folded) -> Self {
        let lead_w: usize = lead.iter().map(|s| s.content.width()).sum();
        Self {
            lead,
            folded,
            content_offset: u16::try_from(lead_w).unwrap_or(u16::MAX),
            decorative: false,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.folded.spans.is_empty()
            && self
                .lead
                .iter()
                .all(|s| s.content.trim().is_empty() || s.content.as_ref() == "│ ")
    }

    pub fn to_spans(&self) -> Vec<Span<'static>> {
        let mut out = Vec::with_capacity(self.lead.len() + self.folded.spans.len());
        out.extend(self.lead.iter().cloned());
        out.extend(self.folded.spans.iter().cloned());
        out
    }
}

pub fn render(text: &str, width: u16) -> Vec<MdLine> {
    let expanded = linebreak::expand_tabs(text, 4);
    let processed = dollars::dollars(&expanded);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES;

    let mut r = Renderer {
        width: width.max(12),
        out: Vec::new(),
        containers: Vec::new(),
        lists: Vec::new(),
        styles: Vec::new(),
        link_starts: Vec::new(),
        pieces: Vec::new(),
        code: None,
        table: None,
        fresh_item: false,
    };

    for event in Parser::new_ext(&processed, options) {
        r.event(event);
    }
    r.finish()
}

enum Container {
    Quote,
    Item { marker: String, used: bool },
}

struct TableState {
    aligns: Vec<Alignment>,
    head: Vec<Cell>,
    rows: Vec<Vec<Cell>>,
    row: Vec<Cell>,
}

const BULLETS: [&str; 3] = ["•", "◦", "▪"];

struct Renderer {
    width: u16,
    out: Vec<MdLine>,
    containers: Vec<Container>,
    lists: Vec<Option<u64>>,
    styles: Vec<Style>,
    link_starts: Vec<(String, usize)>,
    pieces: Vec<Piece>,
    code: Option<(String, String)>,
    table: Option<TableState>,
    fresh_item: bool,
}

impl Renderer {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some((_, body)) => body.push_str(&text),
                None => self.text(&text),
            },
            Event::Code(text) => self.push(&text, theme::md_code()),
            Event::SoftBreak => {
                if self.table.is_some() {
                    self.push(" ", Style::new());
                } else {
                    self.push("\n", Style::new());
                }
            }
            Event::HardBreak => self.push("\n", Style::new()),
            Event::Rule => self.rule(),
            Event::TaskListMarker(done) => {
                if let Some(Container::Item {
                    marker,
                    used: false,
                }) = self.containers.last_mut()
                {
                    *marker = if done { "☑ " } else { "☐ " }.to_string();
                }
            }
            Event::InlineMath(tex) => self.push(&format!("${tex}$"), theme::md_math()),
            Event::DisplayMath(tex) => {
                self.flush();
                self.gap();
                let tex = tex.trim_matches('\n');
                let text = if tex.contains('\n') {
                    format!("$$\n{tex}\n$$")
                } else {
                    format!("$${tex}$$")
                };
                self.emit(vec![Piece::new(text, theme::md_math())], true);
            }
            Event::FootnoteReference(name) => self.push(&format!("[{name}]"), theme::dim()),
            Event::Html(html) | Event::InlineHtml(html) => {
                let h = html.trim();
                if h.eq_ignore_ascii_case("<br>") || h.eq_ignore_ascii_case("<br/>") || h.eq_ignore_ascii_case("<br />") {
                    self.push("\n", Style::new());
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if !std::mem::take(&mut self.fresh_item) {
                    self.flush();
                    self.gap();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.gap();
                let style = match level {
                    HeadingLevel::H1 => theme::md_h1(),
                    HeadingLevel::H2 => theme::md_heading(),
                    _ => theme::md_title(),
                };
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.gap();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.gap();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.gap();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{} ", BULLETS[depth.saturating_sub(1).min(2)]),
                };
                self.containers.push(Container::Item {
                    marker,
                    used: false,
                });
                self.fresh_item = true;
            }
            Tag::Emphasis => self.styles.push(theme::md_italic()),
            Tag::Strong => self.styles.push(theme::md_bold()),
            Tag::Strikethrough => self
                .styles
                .push(Style::new().add_modifier(ratatui::style::Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.styles.push(theme::md_link());
                self.link_starts
                    .push((dest_url.to_string(), self.pieces.len()));
            }
            Tag::Image { dest_url, .. } => {
                self.styles.push(theme::md_image());
                self.link_starts
                    .push((dest_url.to_string(), self.pieces.len()));
                self.push("[图片: ", Style::new());
            }
            Tag::Table(aligns) => {
                self.flush();
                self.gap();
                self.table = Some(TableState {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableCell => {
                self.pieces.clear();
            }
            Tag::FootnoteDefinition(name) => {
                self.flush();
                self.gap();
                self.containers.push(Container::Item {
                    marker: format!("[{name}] "),
                    used: false,
                });
                self.fresh_item = true;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
            }
            TagEnd::CodeBlock => self.code_block(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item | TagEnd::FootnoteDefinition => {
                self.flush();
                self.containers.pop();
                self.fresh_item = false;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some((url, start)) = self.link_starts.pop() {
                    // 如果链接没有可见文本，显示 URL 本身
                    if self.pieces.len() == start {
                        self.pieces
                            .push(Piece::linked(url.clone(), theme::md_url(), &url));
                    }
                }
            }
            TagEnd::Image => {
                self.styles.pop();
                if let Some((url, _)) = self.link_starts.pop() {
                    self.pieces.push(Piece::linked("]", theme::md_image(), &url));
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.pieces);
                if let Some(t) = &mut self.table {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.head = std::mem::take(&mut t.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    let lines = table::render(&t.head, &t.rows, &t.aligns, self.room());
                    for folded in lines {
                        let lead = self.lead(true);
                        self.out.push(MdLine::plain(lead, folded));
                    }
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if !self.link_starts.is_empty() {
            self.push(text, Style::new());
            return;
        }
        let style = self.current_style();
        let mut at = 0;
        for (start, end) in bare_urls(text) {
            self.push(&text[at..start], Style::new());
            let url = &text[start..end];
            self.pieces
                .push(Piece::linked(url, style.patch(theme::md_url()), url));
            at = end;
        }
        self.push(&text[at..], Style::new());
    }

    fn push(&mut self, text: &str, extra: Style) {
        if text.is_empty() {
            return;
        }
        let style = self.current_style().patch(extra);
        let link = self.link_starts.last().map(|(url, _)| url.clone());
        self.pieces.push(Piece {
            text: text.to_string(),
            style,
            link,
        });
    }

    fn current_style(&self) -> Style {
        let quoted = self
            .containers
            .iter()
            .any(|c| matches!(c, Container::Quote));
        let base = if quoted {
            theme::md_quote()
        } else {
            theme::text()
        };
        self.styles.iter().fold(base, |s, next| s.patch(*next))
    }

    fn code_block(&mut self) {
        let Some((lang, body)) = self.code.take() else {
            return;
        };
        let p = theme::palette();
        let code_bg = p.md_code_bg;
        let room = usize::from(self.room());
        // 参考 `sai`：无边框、无竖线装饰符，仅以柔和底色卡片与静谧语言标签区分代码块，
        // 避免划词或终端原生拖选时混入任何装饰字符。
        let left_inset = usize::from(room >= 20);
        let right_inset = usize::from(room >= 20);
        let inner_pad_left = 1usize;
        let inner_pad_right = 1usize;
        let card_w = room.saturating_sub(left_inset + right_inset).max(4);
        let code_w = card_w
            .saturating_sub(inner_pad_left + inner_pad_right)
            .max(2);
        let bg_style = Style::new().bg(code_bg);

        let mut is_first_row = true;
        let lang_trimmed = lang.trim();
        if !lang_trimmed.is_empty() {
            let tag = linebreak::clip(lang_trimmed, code_w as u16);
            let pad = card_w.saturating_sub(inner_pad_left + tag.width());
            let mut spans = Vec::with_capacity(4);
            if left_inset > 0 {
                spans.push(Span::raw(" ".repeat(left_inset)));
            }
            spans.push(Span::styled(" ".repeat(inner_pad_left), bg_style));
            spans.push(Span::styled(tag, theme::faint().bg(code_bg)));
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), bg_style));
            }
            let lead = self.lead(is_first_row);
            let lead_w: usize = lead.iter().map(|s| s.content.width()).sum();
            is_first_row = false;
            self.out.push(MdLine {
                lead,
                folded: Folded {
                    spans,
                    links: Vec::new(),
                    joined: false,
                },
                content_offset: u16::try_from(lead_w + left_inset + inner_pad_left)
                    .unwrap_or(u16::MAX),
                decorative: true,
            });
        }

        let mut hl = code::Highlighter::new(code::find_language(lang_trimmed));
        for raw_line in body.trim_end_matches('\n').split('\n') {
            let pieces = hl.line(raw_line);
            let pieces = if pieces.is_empty() {
                vec![Piece::new("", theme::text())]
            } else {
                pieces
            };
            for folded_line in fold(&pieces, code_w as u16) {
                let mut used_w = 0usize;
                let mut spans = Vec::with_capacity(folded_line.spans.len() + 3);
                if left_inset > 0 {
                    spans.push(Span::raw(" ".repeat(left_inset)));
                }
                spans.push(Span::styled(" ".repeat(inner_pad_left), bg_style));
                for span in folded_line.spans {
                    used_w += span.content.width();
                    spans.push(Span::styled(span.content, span.style.bg(code_bg)));
                }
                let pad = card_w.saturating_sub(inner_pad_left + used_w);
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), bg_style));
                }
                let lead = self.lead(is_first_row);
                let lead_w: usize = lead.iter().map(|s| s.content.width()).sum();
                is_first_row = false;
                self.out.push(MdLine {
                    lead,
                    folded: Folded {
                        spans,
                        links: Vec::new(),
                        joined: folded_line.joined,
                    },
                    content_offset: u16::try_from(lead_w + left_inset + inner_pad_left)
                        .unwrap_or(u16::MAX),
                    decorative: false,
                });
            }
        }
    }

    fn rule(&mut self) {
        self.flush();
        self.gap();
        let w = usize::from(self.room() / 3).clamp(16, 40);
        self.emit(vec![Piece::new("─".repeat(w), theme::faint())], true);
    }

    fn flush(&mut self) {
        let pieces = std::mem::take(&mut self.pieces);
        if !pieces.is_empty() {
            self.emit(pieces, true);
        }
    }

    fn emit(&mut self, pieces: Vec<Piece>, first: bool) {
        let room = self.room();
        for (i, folded) in fold(&pieces, room).into_iter().enumerate() {
            let lead = self.lead(first && i == 0);
            self.out.push(MdLine::plain(lead, folded));
        }
    }

    fn lead(&mut self, first: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for container in &mut self.containers {
            match container {
                Container::Quote => {
                    spans.push(Span::styled("│ ", theme::faint()));
                }
                Container::Item { marker, used } => {
                    let text = if first && !*used {
                        marker.clone()
                    } else {
                        " ".repeat(marker.width())
                    };
                    spans.push(Span::styled(text, theme::md_list()));
                    if first {
                        *used = true;
                    }
                }
            }
        }
        spans
    }

    fn room(&self) -> u16 {
        let lead: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => 2,
                Container::Item { marker, .. } => marker.width(),
            })
            .sum();
        self.width
            .saturating_sub(u16::try_from(lead).unwrap_or(0))
            .max(1)
    }

    fn gap(&mut self) {
        if self.out.last().is_none_or(MdLine::is_blank) {
            return;
        }
        self.out.push(MdLine::plain(Vec::new(), Folded::default()));
    }

    fn finish(mut self) -> Vec<MdLine> {
        self.flush();
        while self.out.last().is_some_and(MdLine::is_blank) {
            self.out.pop();
        }
        self.out
    }
}

fn bare_urls(text: &str) -> Vec<(usize, usize)> {
    const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];
    const TAIL: &str = ".,;:!?'\"。，；：！？、";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some((start, scheme)) = SCHEMES
        .iter()
        .filter_map(|s| text[from..].find(s).map(|i| (from + i, *s)))
        .min_by_key(|(i, _)| *i)
    {
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        let body = text[start..]
            .find(|c: char| {
                !(c.is_ascii_graphic()
                    && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^'))
            })
            .map_or(text.len(), |i| start + i);
        let raw = &text[start..body];
        let mut end_rel = raw.len();
        while let Some(last) = raw[..end_rel].chars().next_back() {
            let opener = match last {
                ')' => Some('('),
                ']' => Some('['),
                _ => None,
            };
            if let Some(open) = opener {
                let slice = &raw[..end_rel];
                if slice.matches(last).count() <= slice.matches(open).count() {
                    break;
                }
            } else if !TAIL.contains(last) {
                break;
            }
            end_rel -= last.len_utf8();
        }
        let end = start + end_rel;
        if !glued && end > start + scheme.len() {
            found.push((start, end));
        }
        from = body.max(start + scheme.len());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::render;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn complex_report_table_and_code_render_cleanly() {
        let sample = "## 修复方案输出\n\n\
            | 字段 | 内容 |\n\
            | --- | --- |\n\
            | **方案属性** | 切中根因方案 — 从源头补齐 `OFF -> LOCKSCREEN` 终态输出 |\n\
            | **改动点** | [OffToLockscreenTransitionViewModel.kt:45-51](file:///D:/Code/Systemui-dev/OffToLockscreenTransitionViewModel.kt) 补充 `onFinish = { 1f }` |\n\n\
            ```kotlin\n\
            val alpha = flowOf(1f) // comment\n\
            ```\n";
        let lines = render(sample, 72);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| {
                l.to_spans()
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let table_lines: Vec<&String> = texts
            .iter()
            .filter(|l| {
                l.starts_with('┌')
                    || l.starts_with('│')
                    || l.starts_with('├')
                    || l.starts_with('└')
            })
            .collect();
        assert!(table_lines.len() >= 5, "should render full boxed table: {texts:#?}");
        let w0 = table_lines[0].width();
        assert!(
            table_lines.iter().all(|l| l.width() == w0),
            "all table borders must align: {table_lines:#?}"
        );
        // 确认 file:/// 超长 URL 不会裸露撑爆表格
        assert!(!texts.iter().any(|l| l.contains("file:///")));
        // 新版 sai 风格代码块：无框线、无 `▎` 装饰符，语言标签行标记为 decorative
        assert!(
            lines.iter().any(|l| l.decorative && l.to_spans().iter().any(|s| s.content.contains("kotlin"))),
            "missing decorative kotlin header: {texts:#?}"
        );
        assert!(!texts.iter().any(|l| l.contains("╭─ code") || l.contains('▎')));
    }
}

