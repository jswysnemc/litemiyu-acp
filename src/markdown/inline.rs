//! 行内样式片段与按显示宽度折行（参考 LiteMiyu `markdown/inline.rs`）。

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::linebreak;

#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
}

impl Piece {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn linked(text: impl Into<String>, style: Style, url: &str) -> Self {
        Self {
            text: text.into(),
            style,
            link: Some(url.to_string()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Folded {
    pub spans: Vec<Span<'static>>,
    pub links: Vec<(u16, u16, String)>,
    #[allow(dead_code)]
    pub joined: bool,
}

impl Folded {
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    fn push(&mut self, text: &str, piece: &Piece) {
        let start = u16::try_from(self.width()).unwrap_or(u16::MAX);
        let end = start.saturating_add(u16::try_from(text.width()).unwrap_or(0));
        match self.spans.last_mut() {
            Some(last) if last.style == piece.style => last.content.to_mut().push_str(text),
            _ => self.spans.push(Span::styled(text.to_string(), piece.style)),
        }
        if let Some(url) = &piece.link {
            match self.links.last_mut() {
                Some((_, to, last)) if last == url && *to == start => *to = end,
                _ => self.links.push((start, end, url.clone())),
            }
        }
    }
}

pub fn fold(pieces: &[Piece], width: u16) -> Vec<Folded> {
    let mut out = Vec::new();
    let mut para: Vec<(&str, usize)> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        for g in piece.text.graphemes(true) {
            if g == "\n" {
                lay(&mut out, &para, pieces, width);
                para.clear();
                continue;
            }
            para.push((g, index));
        }
    }
    lay(&mut out, &para, pieces, width);
    out
}

fn lay(out: &mut Vec<Folded>, para: &[(&str, usize)], pieces: &[Piece], width: u16) {
    let cells: Vec<&str> = para.iter().map(|(g, _)| *g).collect();
    for (k, range) in linebreak::lines(&cells, usize::from(width.max(1)))
        .into_iter()
        .enumerate()
    {
        let mut line = Folded {
            joined: k > 0,
            ..Folded::default()
        };
        for &(g, index) in &para[range] {
            line.push(g, &pieces[index]);
        }
        out.push(line);
    }
}

pub fn plain(folded: &Folded) -> String {
    folded.spans.iter().map(|s| s.content.as_ref()).collect()
}
