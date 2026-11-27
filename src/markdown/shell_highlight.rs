//! Shell 命令（Bash / PowerShell / CMD）精细词法着色器：完整识别 Windows/Linux 路径、URL、重定向、参数标志与环境变量。

use ratatui::style::Style;

use super::inline::Piece;
use super::path_style;
use crate::theme;

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "elif", "else", "fi", "for", "in", "do", "done", "while", "until", "case",
    "esac", "function", "select", "time", "export", "local", "readonly", "unset", "return",
    "break", "continue", "shift", "source", "set", "eval", "exec", "trap", "foreach", "param",
    "begin", "process", "end", "try", "catch", "finally",
];

const EXPECT_CMD_AFTER_KEYWORDS: &[&str] = &[
    "if", "then", "elif", "else", "do", "while", "until", "time", "exec",
];

/// 为运行的终端命令推断最合适的 Shell 语法。
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

/// 【代码高亮】【Shell 命令】对单行 Shell 命令分层着色（命令首词、参数标志、Windows/Linux 路径、URL、字符串、变量、重定向、注释）。
///
/// 参数:
/// - `line`: 单行命令源码
/// - `_lang_name`: 可选语言提示（预留扩展）
///
/// 返回:
/// - 着色后的 `Piece` 切片
pub fn highlight_shell_line(line: &str, _lang_name: Option<&str>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut expect_cmd = true;

    while i < bytes.len() {
        let rest = &line[i..];
        let ch = rest.chars().next().unwrap();

        // 1. 空白字符保持原样
        if ch.is_whitespace() {
            let start = i;
            while i < bytes.len() {
                let c = line[i..].chars().next().unwrap();
                if !c.is_whitespace() {
                    break;
                }
                i += c.len_utf8();
            }
            push_merged(&mut out, &line[start..i], theme::text());
            continue;
        }

        // 2. 行尾注释（`# ...`，位于行首或空白后）
        if ch == '#' && (i == 0 || line[..i].chars().next_back().is_some_and(char::is_whitespace)) {
            push_merged(&mut out, &line[i..], theme::code_comment());
            break;
        }

        // 3. 引号字符串（双引号支持 `$VAR` 插值高亮，单引号/反引号整段着色）
        if ch == '"' || ch == '\'' || ch == '`' {
            let end = scan_quoted_end(line, i, ch);
            if ch == '"' {
                emit_double_quoted(&line[i..end], &mut out);
            } else {
                push_merged(&mut out, &line[i..end], theme::code_string());
            }
            expect_cmd = false;
            i = end;
            continue;
        }

        // 4. 管道、逻辑连接符、分号、分组括号与重定向（如 `&&`, `||`, `|`, `;`, `2>&1`, `>>`, `2>`）
        if let Some((op_end, resets_cmd)) = scan_shell_operator(line, i) {
            push_merged(&mut out, &line[i..op_end], theme::code_operator());
            if resets_cmd {
                expect_cmd = true;
            }
            i = op_end;
            continue;
        }

        // 5. 扫描完整非空白词元（保留路径里的 `/` `\` `:` `~` `-` `.`，不被运算符拆碎）
        let start = i;
        while i < bytes.len() {
            let c = line[i..].chars().next().unwrap();
            if c.is_whitespace()
                || matches!(c, '|' | '&' | ';' | '(' | ')' | '<' | '>' | '"' | '\'' | '`')
            {
                break;
            }
            i += c.len_utf8();
        }
        let token = &line[start..i];
        emit_shell_token(token, &mut expect_cmd, &mut out);
    }

    out
}

/// 根据上下文对单个 Shell 词元执行精细语义着色（环境变量、标志、路径、URL、数字、关键字与命令名）。
fn emit_shell_token(token: &str, expect_cmd: &mut bool, out: &mut Vec<Piece>) {
    if token.is_empty() {
        return;
    }
    let lower = token.to_ascii_lowercase();

    // 1. 变量引用（`$VAR`、`${VAR}`、`$env:PATH`）
    if token.starts_with('$') {
        push_merged(out, token, theme::code_macro());
        *expect_cmd = false;
        return;
    }

    // 2. Shell 关键字
    if *expect_cmd && SHELL_KEYWORDS.contains(&lower.as_str()) {
        push_merged(out, token, theme::code_keyword());
        *expect_cmd = EXPECT_CMD_AFTER_KEYWORDS.contains(&lower.as_str());
        return;
    }

    // 3. 环境变量赋值 `FOO=value`（位于命令首部时后续仍等待真正命令名）
    if *expect_cmd
        && let Some((k, v)) = token.split_once('=')
        && !k.is_empty()
        && !k.starts_with('-')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        push_merged(out, k, theme::code_property());
        push_merged(out, "=", theme::code_operator());
        emit_value_token(v, out);
        return;
    }

    // 4. 命令行标志（`--release`、`-m`、`-Filter`、`--path=D:\foo\bar`）
    if token.starts_with('-') && token.len() >= 2 && !token.chars().nth(1).unwrap().is_ascii_digit() {
        if let Some((flag, val)) = token.split_once('=') {
            push_merged(out, flag, theme::code_parameter());
            push_merged(out, "=", theme::code_operator());
            emit_value_token(val, out);
        } else {
            push_merged(out, token, theme::code_parameter());
        }
        *expect_cmd = false;
        return;
    }

    // 5. 命令首词（含路径形式的可执行程序如 `./target/debug/app`、`C:\bin\tool.exe`）
    if *expect_cmd {
        *expect_cmd = false;
        if path_style::looks_like_file_path(token) {
            for p in path_style::style_path_pieces(token, None) {
                push_merged(out, &p.text, p.style);
            }
        } else {
            push_merged(out, token, theme::code_function());
        }
        return;
    }

    // 6. 普通参数值：识别 URL、Windows/Linux 路径、数字、普通文本
    emit_value_token(token, out);
}

fn emit_value_token(val: &str, out: &mut Vec<Piece>) {
    if val.is_empty() {
        return;
    }
    if val.starts_with("https://") || val.starts_with("http://") || val.starts_with("file://") {
        push_merged(out, val, theme::md_url());
    } else if val.starts_with('$') {
        push_merged(out, val, theme::code_macro());
    } else if path_style::looks_like_file_path(val) {
        for p in path_style::style_path_pieces(val, None) {
            push_merged(out, &p.text, p.style);
        }
    } else if val.chars().all(|c| c.is_ascii_digit() || c == '.') && val.chars().any(|c| c.is_ascii_digit()) {
        push_merged(out, val, theme::code_number());
    } else if matches!(val, "true" | "false" | "null" | "none" | "True" | "False") {
        push_merged(out, val, theme::code_constant());
    } else {
        push_merged(out, val, theme::text());
    }
}

fn scan_shell_operator(line: &str, i: usize) -> Option<(usize, bool)> {
    let rest = &line[i..];
    // 处理带文件描述符的重定向（如 `2>&1`、`2>>`、`1>`）
    let b = rest.as_bytes();
    if b.len() >= 2
        && b[0].is_ascii_digit()
        && (b[1] == b'>' || b[1] == b'<')
        && (i == 0 || line[..i].chars().next_back().is_some_and(char::is_whitespace))
    {
        let mut len = 2;
        if rest.as_bytes().get(len) == Some(&b'>') || rest.as_bytes().get(len) == Some(&b'&') {
            len += 1;
            if rest.as_bytes().get(len).is_some_and(u8::is_ascii_digit) {
                len += 1;
            }
        }
        return Some((i + len, false));
    }
    for (op, resets) in [
        ("&&", true),
        ("||", true),
        ("|&", true),
        (">>", false),
        ("<<", false),
        (">&", false),
        ("&>", false),
        ("|", true),
        (";", true),
        ("(", true),
        (")", false),
        ("&", true),
        (">", false),
        ("<", false),
    ] {
        if rest.starts_with(op) {
            return Some((i + op.len(), resets));
        }
    }
    None
}

fn scan_quoted_end(line: &str, start: usize, quote: char) -> usize {
    let bytes = line.as_bytes();
    let mut i = start + quote.len_utf8();
    while i < bytes.len() {
        let c = line[i..].chars().next().unwrap();
        if c == '\\' && quote == '"' && i + 1 < bytes.len() {
            let next = line[i + 1..].chars().next().unwrap();
            i += 1 + next.len_utf8();
            continue;
        }
        i += c.len_utf8();
        if c == quote {
            return i;
        }
    }
    bytes.len()
}

fn emit_double_quoted(seg: &str, out: &mut Vec<Piece>) {
    let bytes = seg.as_bytes();
    let mut from = 0usize;
    let mut i = 1usize;
    while i < bytes.len() {
        let c = seg[i..].chars().next().unwrap();
        if c == '\\' && i + 1 < bytes.len() {
            let next = seg[i + 1..].chars().next().unwrap();
            i += 1 + next.len_utf8();
            continue;
        }
        if c == '$' && i + 1 < bytes.len() {
            let next = seg[i + 1..].chars().next().unwrap();
            if next == '{' || next == '(' || next.is_ascii_alphanumeric() || next == '_' {
                push_merged(out, &seg[from..i], theme::code_string());
                let mut end = i + 1 + next.len_utf8();
                if next == '{' || next == '(' {
                    let close = if next == '{' { '}' } else { ')' };
                    if let Some(pos) = seg[end..].find(close) {
                        end += pos + 1;
                    } else {
                        end = bytes.len();
                    }
                } else {
                    while end < bytes.len() {
                        let nc = seg[end..].chars().next().unwrap();
                        if !(nc.is_ascii_alphanumeric() || nc == '_' || nc == ':') {
                            break;
                        }
                        end += nc.len_utf8();
                    }
                }
                push_merged(out, &seg[i..end], theme::code_macro());
                from = end;
                i = end;
                continue;
            }
        }
        i += c.len_utf8();
    }
    push_merged(out, &seg[from..], theme::code_string());
}

fn push_merged(out: &mut Vec<Piece>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == style
        && last.link.is_none()
    {
        last.text.push_str(text);
        return;
    }
    out.push(Piece::new(text, style));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, ThemePreset};

    #[test]
    fn highlights_windows_and_linux_paths_in_shell_commands() {
        theme::init(ThemePreset::MiyuLavender, true);
        let linux_cmd = highlight_shell_line(
            "rg --line-number foo /home/snemc/workspace/src/main.rs 2>&1",
            Some("bash"),
        );
        assert!(linux_cmd.iter().any(|p| p.text == "rg" && p.style == theme::code_function()));
        assert!(linux_cmd.iter().any(|p| p.text == "--line-number" && p.style == theme::code_parameter()));
        assert!(linux_cmd.iter().any(|p| p.text == "main.rs" && p.style == theme::md_link()));
        assert!(linux_cmd.iter().any(|p| p.text == "2>&1" && p.style == theme::code_operator()));

        let win_cmd = highlight_shell_line(
            r"Get-Content -Path D:\Code\Systemui-dev\Foo.kt",
            Some("powershell"),
        );
        assert!(win_cmd.iter().any(|p| p.text == "Get-Content" && p.style == theme::code_function()));
        assert!(win_cmd.iter().any(|p| p.text == "D:" && p.style == theme::md_url()));
        assert!(win_cmd.iter().any(|p| p.text == "Foo.kt" && p.style == theme::md_link()));
    }
}
