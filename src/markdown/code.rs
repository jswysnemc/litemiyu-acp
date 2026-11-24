//! 轻量多语言词法高亮器（参考 LiteMiyu `markdown/code/` + 内嵌 `resources/code.json`）。

use std::collections::HashSet;
use std::sync::OnceLock;

use ratatui::style::Style;
use serde::Deserialize;

use super::inline::Piece;
use crate::theme;

const CODE_JSON: &str = include_str!("../../resources/code.json");

#[derive(Deserialize)]
struct CodeFile {
    languages: Vec<Language>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Language {
    pub names: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub constants: Vec<String>,
    #[serde(default)]
    pub capital_types: bool,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub block_comment: Option<[String; 2]>,
    #[serde(default)]
    pub single_quote_strings: bool,
    #[serde(default)]
    pub backtick_strings: bool,
    #[serde(default)]
    pub triple_quote_strings: bool,
    #[serde(default)]
    pub macros: bool,
    #[serde(default)]
    pub attributes: bool,
    #[serde(default)]
    pub preprocessor: bool,
    #[serde(default)]
    pub decorators: bool,
    #[serde(default)]
    pub variables: bool,
    #[serde(default)]
    pub flags: bool,
    #[serde(default)]
    pub keys: String,
    #[serde(default)]
    pub sections: bool,
    #[serde(default)]
    pub tags: bool,
    #[serde(default)]
    pub diff: bool,
}

static LANGUAGES: OnceLock<Vec<Language>> = OnceLock::new();

pub fn find_language(name: &str) -> Option<&'static Language> {
    let key = name.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }
    let all = LANGUAGES.get_or_init(|| {
        serde_json::from_str::<CodeFile>(CODE_JSON)
            .map(|f| f.languages)
            .unwrap_or_default()
    });
    all.iter()
        .find(|l| l.names.iter().any(|n| n.eq_ignore_ascii_case(&key)))
}

/// 为运行的终端命令推断最合适的 Shell 语法（参考 `sai/src/render/command_highlight.rs`）。
pub fn detect_shell_language(command: &str) -> &'static str {
    let lower = command.to_ascii_lowercase();
    let looks_powershell = lower.contains("get-")
        || lower.contains("set-")
        || lower.contains("select-object")
        || lower.contains("where-object")
        || lower.contains("foreach-object")
        || lower.contains("convertfrom-")
        || lower.contains("convertto-")
        || lower.contains("write-host")
        || lower.contains("$env:")
        || lower.contains("pwsh")
        || lower.contains("powershell");
    let looks_posix = lower.contains("export ")
        || lower.contains("grep ")
        || lower.contains("awk ")
        || lower.contains("sed ")
        || lower.starts_with("#!/");
    if looks_powershell {
        "powershell"
    } else if looks_posix {
        "bash"
    } else if cfg!(windows) {
        "powershell"
    } else {
        "bash"
    }
}

/// 对单行 Shell 命令做语法着色（参考 `sai`：命令名、参数标志、字符串、变量、数字、管道与操作符分层着色）。
pub fn highlight_shell_line(line: &str, lang_name: Option<&str>) -> Vec<Piece> {
    let lang = lang_name
        .unwrap_or_else(|| detect_shell_language(line))
        .to_string();
    let mut hl = Highlighter::new(find_language(&lang).or_else(|| find_language("bash")));
    let raw = hl.line(line);
    emphasize_command_heads(raw)
}

/// 将每段命令首位的可执行程序/Cmdlet（行首或 `|`, `&&`, `||`, `;`, `(` 之后）高亮为函数色。
fn emphasize_command_heads(pieces: Vec<Piece>) -> Vec<Piece> {
    let plain = theme::text();
    let cmd_style = theme::code_function();
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len() + 4);
    let mut expect_cmd = true;
    let mut in_cmd_word = false;

    for piece in pieces {
        if piece.style == theme::code_comment() {
            out.push(piece);
            continue;
        }
        if piece.style == theme::code_operator() {
            let trimmed = piece.text.trim();
            if in_cmd_word && trimmed == "-" {
                // 连字符命令名（如 `Get-ChildItem`、`docker-compose`）延续命令高亮
                out.push(Piece::new(piece.text, cmd_style));
                continue;
            }
            if trimmed.contains('|')
                || trimmed.contains('&')
                || trimmed.contains(';')
                || trimmed == "&&"
                || trimmed == "||"
            {
                expect_cmd = true;
                in_cmd_word = false;
            } else {
                in_cmd_word = false;
            }
            out.push(piece);
            continue;
        }

        if piece.style == plain {
            // 将普通文本按空白和命令分隔符切片，精准标亮命令首词
            let mut idx = 0;
            let bytes = piece.text.as_bytes();
            while idx < bytes.len() {
                let ch = piece.text[idx..].chars().next().unwrap_or(' ');
                if ch.is_whitespace() {
                    let start = idx;
                    while idx < bytes.len() {
                        let c = piece.text[idx..].chars().next().unwrap_or(' ');
                        if !c.is_whitespace() {
                            break;
                        }
                        idx += c.len_utf8();
                    }
                    in_cmd_word = false;
                    push_merged(&mut out, &piece.text[start..idx], plain);
                } else if matches!(ch, ';' | '(' | '{') {
                    let end = idx + ch.len_utf8();
                    push_merged(&mut out, &piece.text[idx..end], theme::code_operator());
                    expect_cmd = true;
                    in_cmd_word = false;
                    idx = end;
                } else {
                    let start = idx;
                    while idx < bytes.len() {
                        let c = piece.text[idx..].chars().next().unwrap_or(' ');
                        if c.is_whitespace() || matches!(c, ';' | '(' | '{') {
                            break;
                        }
                        idx += c.len_utf8();
                    }
                    let token = &piece.text[start..idx];
                    if expect_cmd || in_cmd_word {
                        // 环境变量赋值 `FOO=bar` 后仍等待真正命令名
                        let is_env_assign = token.contains('=') && !token.starts_with('=');
                        let st = if is_env_assign {
                            theme::code_property()
                        } else {
                            expect_cmd = false;
                            in_cmd_word = true;
                            cmd_style
                        };
                        push_merged(&mut out, token, st);
                    } else {
                        push_merged(&mut out, token, plain);
                    }
                }
            }
            continue;
        }

        // 其他已着色 token（如关键字、参数、字符串、变量）出现后，结束当前命令首词期待
        if piece.style == theme::code_keyword() {
            // `if` / `then` / `do` / `else` 后面的下一个词往往仍是命令
            let kw = piece.text.trim().to_ascii_lowercase();
            expect_cmd = matches!(kw.as_str(), "if" | "then" | "elif" | "else" | "do" | "while" | "until");
            in_cmd_word = false;
        } else if !in_cmd_word {
            expect_cmd = false;
        } else {
            in_cmd_word = false;
        }
        push_merged(&mut out, &piece.text, piece.style);
    }
    out
}

fn push_merged(out: &mut Vec<Piece>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.style == style && last.link.is_none() {
            last.text.push_str(text);
            return;
        }
    }
    out.push(Piece::new(text, style));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    Comment,
    Triple(char),
}

pub struct Highlighter {
    language: Option<&'static Language>,
    keywords: HashSet<String>,
    types: HashSet<&'static str>,
    constants: HashSet<&'static str>,
    state: State,
}

impl Highlighter {
    pub fn new(language: Option<&'static Language>) -> Self {
        let fold = |w: &str| {
            if language.is_some_and(|l| l.ignore_case) {
                w.to_lowercase()
            } else {
                w.to_string()
            }
        };
        Self {
            language,
            keywords: language.map_or_else(HashSet::new, |l| {
                l.keywords.iter().map(|k| fold(k)).collect()
            }),
            types: language.map_or_else(HashSet::new, |l| {
                l.types.iter().map(String::as_str).collect()
            }),
            constants: language.map_or_else(HashSet::new, |l| {
                l.constants.iter().map(String::as_str).collect()
            }),
            state: State::Code,
        }
    }

    pub fn line(&mut self, line: &str) -> Vec<Piece> {
        if self.language.is_none() {
            return if line.is_empty() {
                Vec::new()
            } else {
                vec![Piece::new(line, theme::text())]
            };
        }
        if self.language.is_some_and(|l| l.diff) {
            return diff_line(line);
        }
        self.scan(line)
    }

    fn is_keyword(&self, word: &str) -> bool {
        if self.language.is_some_and(|l| l.ignore_case) {
            self.keywords.contains(&word.to_lowercase())
        } else {
            self.keywords.contains(word)
        }
    }

    fn scan(&mut self, text: &str) -> Vec<Piece> {
        let line = Line {
            text,
            chars: text.char_indices().collect(),
        };
        let mut out = Out::default();
        let Some(mut i) = self.resume(&line, &mut out) else {
            return out.0;
        };
        let lang = self.language;
        if i == 0 && lang.is_some_and(|l| l.sections) {
            let body = text.trim();
            if body.starts_with('[') && body.ends_with(']') {
                let lead = text.len() - text.trim_start().len();
                out.push(&text[..lead], theme::text());
                out.push(body, theme::code_type());
                out.push(&text[lead + body.len()..], theme::text());
                return out.0;
            }
        }
        let mut in_tag = false;
        let variables = lang.is_some_and(|l| l.variables);
        while i < line.chars.len() {
            if variables && line.char(i) == Some('"') {
                i = interpolated(&line, i, &mut out);
                continue;
            }
            match self.token(&line, i, &mut in_tag) {
                Some((end, style)) => {
                    out.push(line.slice(i, end), style);
                    i = end;
                }
                None => {
                    let (rest, style) = self.until_end(&line, i);
                    out.push(rest, style);
                    break;
                }
            }
        }
        out.0
    }

    fn resume(&mut self, line: &Line, out: &mut Out) -> Option<usize> {
        let (close, style) = match self.state {
            State::Code => return Some(0),
            State::Comment => (
                self.language
                    .and_then(|l| l.block_comment.as_ref())
                    .map_or(String::new(), |[_, close]| close.clone()),
                theme::code_comment(),
            ),
            State::Triple(q) => (q.to_string().repeat(3), theme::code_string()),
        };
        match line.text.find(close.as_str()).filter(|_| !close.is_empty()) {
            Some(at) => {
                let stop = at + close.len();
                out.push(&line.text[..stop], style);
                self.state = State::Code;
                Some(line.index(stop))
            }
            None => {
                out.push(line.text, style);
                None
            }
        }
    }

    fn token(&mut self, line: &Line, i: usize, in_tag: &mut bool) -> Option<(usize, Style)> {
        let lang = self.language;
        let rest = &line.text[line.byte(i)..];
        let c = line.char(i).unwrap_or(' ');
        let prev = i.checked_sub(1).and_then(|p| line.char(p));
        if let Some([open, close]) = lang.and_then(|l| l.block_comment.as_ref()) {
            if rest.starts_with(open.as_str()) {
                let Some(at) = rest[open.len()..].find(close.as_str()) else {
                    self.state = State::Comment;
                    return None;
                };
                let stop = line.byte(i) + open.len() + at + close.len();
                return Some((line.index(stop), theme::code_comment()));
            }
        }
        if let Some(l) = lang {
            let attribute = l.attributes && (rest.starts_with("#[") || rest.starts_with("#!["));
            let spaced = l.comment != "#" || prev.is_none_or(char::is_whitespace);
            if !l.comment.is_empty() && rest.starts_with(&l.comment) && spaced && !attribute {
                return None;
            }
            if attribute {
                let end = bracket_end(rest);
                return Some((line.index(line.byte(i) + end), theme::code_macro()));
            }
        }
        if c == '#'
            && lang.is_some_and(|l| l.preprocessor)
            && line.prev_solid(i).is_none()
            && line.char(i + 1).is_some_and(is_word)
        {
            return Some((line.word_end(i + 1), theme::code_macro()));
        }
        if c == '@' && lang.is_some_and(|l| l.decorators) && line.char(i + 1).is_some_and(is_word)
        {
            return Some((line.word_end(i + 1), theme::code_macro()));
        }
        if c == '$' && lang.is_some_and(|l| l.variables) {
            return Some((variable_end(line, i), theme::code_macro()));
        }
        if lang.is_some_and(|l| l.tags) {
            if c == '<' && line.char(i + 1).is_some_and(|n| is_word(n) || n == '/') {
                *in_tag = true;
                let skip = if line.char(i + 1) == Some('/') { 2 } else { 1 };
                return Some((i + skip, theme::code_operator()));
            }
            if *in_tag && c == '>' {
                *in_tag = false;
                return Some((i + 1, theme::code_operator()));
            }
        }
        self.literal(line, i, c, prev, *in_tag)
    }

    fn literal(
        &mut self,
        line: &Line,
        i: usize,
        c: char,
        prev: Option<char>,
        in_tag: bool,
    ) -> Option<(usize, Style)> {
        let lang = self.language;
        let rest = &line.text[line.byte(i)..];
        if lang.is_some_and(|l| l.triple_quote_strings)
            && (rest.starts_with("\"\"\"") || rest.starts_with("'''"))
        {
            let close = c.to_string().repeat(3);
            let Some(at) = rest[3..].find(close.as_str()) else {
                self.state = State::Triple(c);
                return None;
            };
            return Some((line.index(line.byte(i) + 3 + at + 3), theme::code_string()));
        }
        let quote = c == '"'
            || (c == '\'' && lang.is_some_and(|l| l.single_quote_strings))
            || (c == '`' && lang.is_some_and(|l| l.backtick_strings));
        if quote {
            let end = string_end(line, i, c);
            let style = if self.key_here(line, i, end) {
                theme::code_property()
            } else {
                theme::code_string()
            };
            return Some((end, style));
        }
        if c.is_ascii_digit() && !prev.is_some_and(is_word) {
            let end = (i..line.chars.len())
                .find(|&j| {
                    !matches!(
                        line.chars[j].1,
                        ch if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'
                    )
                })
                .unwrap_or(line.chars.len());
            return Some((end, theme::code_number()));
        }
        if c == '-'
            && lang.is_some_and(|l| l.flags)
            && prev.is_none_or(char::is_whitespace)
            && line
                .char(i + 1)
                .is_some_and(|n| n == '-' || n.is_alphabetic())
        {
            let end = (i..line.chars.len())
                .find(|&j| line.chars[j].1.is_whitespace() || line.chars[j].1 == '=')
                .unwrap_or(line.chars.len());
            return Some((end, theme::code_parameter()));
        }
        if is_word(c) {
            let end = line.word_end(i);
            return Some(self.word(line, i, end, in_tag));
        }
        if is_operator(c) {
            let end = (i..line.chars.len())
                .find(|&j| !is_operator(line.chars[j].1))
                .unwrap_or(line.chars.len());
            return Some((end, theme::code_operator()));
        }
        Some((i + 1, theme::text()))
    }

    fn word(&self, line: &Line, i: usize, end: usize, in_tag: bool) -> (usize, Style) {
        let word = line.slice(i, end);
        let lang = self.language;
        let bang = line.char(end) == Some('!') && line.char(end + 1) != Some('=');
        if bang && lang.is_some_and(|l| l.macros) {
            return (end + 1, theme::code_macro());
        }
        let next = line.next_solid(end);
        if (in_tag && next == Some('=')) || self.key_here(line, i, end) {
            return (end, theme::code_property());
        }
        if in_tag && matches!(line.prev_solid(i), Some('<' | '/')) {
            return (end, theme::code_keyword());
        }
        let style = if self.constants.contains(word) {
            theme::code_constant()
        } else if self.is_keyword(word) {
            theme::code_keyword()
        } else if self.types.contains(word) {
            theme::code_type()
        } else if shouting(word) {
            theme::code_constant()
        } else if next == Some('(') {
            theme::code_function()
        } else if lang.is_some_and(|l| l.capital_types) && capitalized(word) {
            theme::code_type()
        } else {
            theme::text()
        };
        (end, style)
    }

    fn key_here(&self, line: &Line, i: usize, end: usize) -> bool {
        let Some(seps) = self
            .language
            .map(|l| l.keys.as_str())
            .filter(|k| !k.is_empty())
        else {
            return false;
        };
        let opens = line
            .prev_solid(i)
            .is_none_or(|p| matches!(p, '{' | ',' | '-'));
        let next = line.next_solid(end);
        opens && next.is_some_and(|n| seps.contains(n)) && line.char(end + 1) != Some(':')
    }

    fn until_end<'l>(&self, line: &Line<'l>, i: usize) -> (&'l str, Style) {
        let style = match self.state {
            State::Triple(_) => theme::code_string(),
            _ => theme::code_comment(),
        };
        (&line.text[line.byte(i)..], style)
    }
}

#[derive(Default)]
struct Out(Vec<Piece>);

impl Out {
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.0.push(Piece::new(text, style)),
        }
    }
}

struct Line<'l> {
    text: &'l str,
    chars: Vec<(usize, char)>,
}

impl Line<'_> {
    fn byte(&self, i: usize) -> usize {
        self.chars.get(i).map_or(self.text.len(), |(b, _)| *b)
    }
    fn index(&self, b: usize) -> usize {
        self.chars.partition_point(|(at, _)| *at < b)
    }
    fn char(&self, i: usize) -> Option<char> {
        self.chars.get(i).map(|(_, c)| *c)
    }
    fn slice(&self, from: usize, to: usize) -> &str {
        &self.text[self.byte(from)..self.byte(to)]
    }
    fn word_end(&self, i: usize) -> usize {
        (i..self.chars.len())
            .find(|&j| !is_word(self.chars[j].1))
            .unwrap_or(self.chars.len())
    }
    fn next_solid(&self, i: usize) -> Option<char> {
        self.chars[i.min(self.chars.len())..]
            .iter()
            .map(|(_, c)| *c)
            .find(|c| !c.is_whitespace())
    }
    fn prev_solid(&self, i: usize) -> Option<char> {
        self.chars[..i]
            .iter()
            .rev()
            .map(|(_, c)| *c)
            .find(|c| !c.is_whitespace())
    }
}

fn diff_line(line: &str) -> Vec<Piece> {
    if line.is_empty() {
        return Vec::new();
    }
    let style = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
        theme::code_macro()
    } else if line.starts_with('+') {
        theme::added()
    } else if line.starts_with('-') {
        theme::removed()
    } else {
        theme::text()
    };
    vec![Piece::new(line, style)]
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '+' | '-' | '*' | '/' | '%' | '<' | '>' | '!' | '&' | '|' | '^' | '~' | '?' | ':'
    )
}

fn shouting(word: &str) -> bool {
    word.chars().count() >= 2
        && word.chars().any(char::is_alphabetic)
        && !word.chars().any(char::is_lowercase)
}

fn capitalized(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_uppercase) && word.chars().any(char::is_lowercase)
}

fn string_end(line: &Line, start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < line.chars.len() {
        match line.chars[i].1 {
            '\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    line.chars.len()
}

fn interpolated(line: &Line, i: usize, out: &mut Out) -> usize {
    let end = string_end(line, i, '"');
    let mut from = i;
    let mut j = i + 1;
    while j < end {
        match line.char(j) {
            Some('\\') => j += 2,
            Some('$') if j + 1 < end => {
                out.push(line.slice(from, j), theme::code_string());
                let stop = variable_end(line, j).min(end);
                out.push(line.slice(j, stop), theme::code_macro());
                from = stop;
                j = stop.max(j + 1);
            }
            _ => j += 1,
        }
    }
    out.push(line.slice(from, end), theme::code_string());
    end
}

fn bracket_end(rest: &str) -> usize {
    let mut depth = 0;
    for (at, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    rest.len()
}

fn variable_end(line: &Line, i: usize) -> usize {
    match line.char(i + 1) {
        Some(open @ ('{' | '(')) => {
            let close = if open == '{' { '}' } else { ')' };
            (i + 2..line.chars.len())
                .find(|&j| line.chars[j].1 == close)
                .map_or(line.chars.len(), |j| j + 1)
        }
        Some(c) if is_word(c) => line.word_end(i + 1),
        Some('@' | '#' | '?' | '*' | '!' | '$' | '-') => i + 2,
        _ => i + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, ThemePreset};

    #[test]
    fn highlights_shell_command_heads_flags_operators_and_strings() {
        theme::init(ThemePreset::MiyuLavender, true);
        let pieces = highlight_shell_line(
            r#"cargo test --release && git commit -m "fix bug" # done"#,
            Some("bash"),
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text == "cargo" && p.style == theme::code_function()),
            "cargo should be highlighted as command head: {pieces:#?}"
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text == "git" && p.style == theme::code_function()),
            "git after && should be highlighted as command head: {pieces:#?}"
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text == "--release" && p.style == theme::code_parameter()),
            "--release should be highlighted as parameter flag: {pieces:#?}"
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text.contains("fix bug") && p.style == theme::code_string()),
            "string should be highlighted: {pieces:#?}"
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text == "&&" && p.style == theme::code_operator()),
            "&& should be highlighted as operator: {pieces:#?}"
        );
        assert!(
            pieces
                .iter()
                .any(|p| p.text.contains("# done") && p.style == theme::code_comment()),
            "comment should be highlighted: {pieces:#?}"
        );
    }
}
