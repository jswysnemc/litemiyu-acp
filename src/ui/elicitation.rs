//! ACP `elicitation/create`（含 `agy ask_question`）交互式提问表单抽屉渲染。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::acp::ElicitationFieldKind;
use crate::app::ElicitationState;
use crate::linebreak;
use crate::theme;

use super::HoverTarget;
use super::overlay_hits::{ElicitationHits, OverlayFooterAction, build_right_hint_line};

/// 计算提问抽屉在当前主栏高度下应占用的合理行数。
///
/// 参数:
/// - `state`: 活跃中的提问状态
/// - `max_h`: 主栏最大可用高度
///
/// 返回:
/// - 抽屉边框总高度（含上下边框）
pub fn drawer_height(state: &ElicitationState, max_h: u16) -> u16 {
    let body_rows = if state.req.mode == "url" || state.req.fields.is_empty() {
        4u16
    } else {
        let active = state.active_field.min(state.req.fields.len() - 1);
        let field = &state.req.fields[active];
        let mut rows = 2u16; // 问题标题 + 空行
        if state.req.fields.len() > 1 {
            rows += 1; // 多题页签条
        }
        if !field.description.is_empty() {
            rows += 1;
        }
        rows += match &field.kind {
            ElicitationFieldKind::SingleSelect(opts)
            | ElicitationFieldKind::MultiSelect { options: opts, .. } => {
                opts.len() as u16 + u16::from(field.other_key.is_some())
            }
            ElicitationFieldKind::Boolean { .. } => 2,
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. } => 2,
        };
        rows
    };
    (body_rows + 2)
        .clamp(7, (max_h / 2).max(7))
        .min(max_h.saturating_sub(3).max(5))
}

/// 在输入框区域绘制 `elicitation/create` 交互式提问/表单抽屉，并返回各页签与选项的鼠标热区。
///
/// 参数:
/// - `frame`: ratatui 渲染帧
/// - `area`: 抽屉外框区域
/// - `state`: 当前提问表单状态
///
/// 返回:
/// - 抽屉外框、多题页签、选项行及链接的鼠标热区
pub fn draw_elicitation_drawer(
    frame: &mut Frame,
    area: Rect,
    state: &ElicitationState,
    hover: &HoverTarget,
) -> ElicitationHits {
    let mut hits = ElicitationHits {
        area,
        ..ElicitationHits::default()
    };
    let p = theme::palette();
    frame.render_widget(Clear, area);

    let field_count = state.req.fields.len();
    let title = if field_count > 1 {
        format!(" ◈ 代理提问 ({}/{}) ", state.active_field + 1, field_count)
    } else {
        " ◈ 代理提问 ".to_string()
    };

    let bottom_y = area.bottom().saturating_sub(1);
    let (hint_line, footer_actions) = if state.req.mode == "url" || field_count == 0 {
        let mut segs: Vec<(&str, Option<OverlayFooterAction>)> =
            vec![("Enter 确认", Some(OverlayFooterAction::Confirm))];
        if state.req.url.is_some() {
            segs.push((" · ", None));
            segs.push(("O 打开链接", Some(OverlayFooterAction::OpenUrl)));
        }
        segs.push((" · ", None));
        segs.push(("Esc 取消", Some(OverlayFooterAction::Close)));
        build_right_hint_line(area, bottom_y, &segs, hover)
    } else {
        let active = state.active_field.min(field_count - 1);
        let field = &state.req.fields[active];
        let is_editing = state.answers[active].editing_text;
        let prefix = match (&field.kind, is_editing, field_count > 1) {
            (ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. }, _, true) => {
                "输入回答 · Tab 切题 · "
            }
            (ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. }, _, false) => {
                "输入回答 · "
            }
            (_, true, _) => "输入自定义内容 · ",
            (ElicitationFieldKind::MultiSelect { .. }, false, _) => "点击/Space 勾选 · ",
            (_, false, true) => "点击/↑↓ 选择 · ←→ 切题 · ",
            (_, false, false) => "点击/↑↓ 选择 · ",
        };
        build_right_hint_line(
            area,
            bottom_y,
            &[
                (prefix, None),
                ("Enter 提交", Some(OverlayFooterAction::Confirm)),
                (" · ", None),
                ("Esc 取消", Some(OverlayFooterAction::Close)),
            ],
            hover,
        )
    };
    hits.footer_actions = footer_actions;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title_top(Line::styled(title, theme::accent_bold()))
        .title_bottom(hint_line)
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 6 || inner.height == 0 {
        return hits;
    }

    if state.req.mode == "url" || field_count == 0 {
        let mut lines = vec![Line::styled(
            linebreak::clip(&state.req.message, inner.width),
            theme::text(),
        )];
        if let Some(url) = &state.req.url {
            lines.push(Line::default());
            let url_y = inner.y.saturating_add(lines.len() as u16);
            if url_y < inner.bottom() {
                hits.url_row = Some(Rect::new(inner.x, url_y, inner.width, 1));
            }
            lines.push(Line::styled(
                linebreak::clip(url, inner.width),
                theme::md_url(),
            ));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return hits;
    }

    let active = state.active_field.min(field_count - 1);
    let field = &state.req.fields[active];
    let ans = &state.answers[active];
    let max_w = usize::from(inner.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_pos: Option<(u16, u16)> = None;
    let mut raw_tabs: Vec<(u16, u16, usize)> = Vec::new();
    let mut raw_options: Vec<(usize, usize, bool)> = Vec::new();

    // 1. 多题导航页签（记录每个页签横向坐标，供鼠标单击切题）
    if field_count > 1 {
        let mut tab_spans: Vec<Span<'static>> = Vec::new();
        let mut tab_x = inner.x;
        for i in 0..field_count {
            let done = state.is_field_answered(i);
            let mark = if done { "✓" } else { "·" };
            let label = format!(" {mark} 问题{} ", i + 1);
            let label_w = u16::try_from(label.width()).unwrap_or(0);
            if tab_x < inner.right() && label_w > 0 {
                let w = label_w.min(inner.right().saturating_sub(tab_x));
                raw_tabs.push((tab_x, w, i));
            }
            tab_x = tab_x.saturating_add(label_w).saturating_add(1);
            let style = if i == active {
                theme::picked_bar()
            } else if done {
                theme::good()
            } else {
                theme::dim()
            };
            tab_spans.push(Span::styled(label, style));
            tab_spans.push(Span::raw(" "));
        }
        lines.push(Line::from(tab_spans));
    }

    // 2. 问题标题与补充描述
    let req_mark = if field.required { "* " } else { "" };
    lines.push(Line::from(vec![
        Span::styled(req_mark, theme::warn()),
        Span::styled(
            linebreak::clip(&field.title, inner.width.saturating_sub(2)),
            theme::accent_bold(),
        ),
    ]));
    if !field.description.is_empty() {
        lines.push(Line::styled(
            linebreak::clip(&field.description, inner.width),
            theme::dim(),
        ));
    }

    // 3. 选项 / 文本输入区
    match &field.kind {
        ElicitationFieldKind::SingleSelect(opts) => {
            for (idx, opt) in opts.iter().enumerate() {
                raw_options.push((lines.len(), idx, false));
                let is_cur = !ans.editing_text && ans.cursor_opt == idx;
                let is_picked = ans.selected_idx == Some(idx);
                let mark = if is_picked { "●" } else { "○" };
                let num = idx + 1;
                let desc = if opt.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", opt.description)
                };
                let label = format!(" {mark} [{num}] {}{desc}", opt.label);
                let clipped = linebreak::clip(&label, inner.width);
                if is_cur {
                    let pad = max_w.saturating_sub(clipped.width());
                    lines.push(Line::styled(
                        format!("{clipped}{}", " ".repeat(pad)),
                        theme::picked_bar(),
                    ));
                } else {
                    let st = if is_picked {
                        theme::good()
                    } else {
                        theme::text()
                    };
                    lines.push(Line::styled(clipped, st));
                }
            }
            if field.other_key.is_some() {
                raw_options.push((lines.len(), opts.len(), true));
                let is_cur = ans.editing_text || ans.cursor_opt == opts.len();
                let row_y = inner.y + lines.len() as u16;
                cursor_pos = push_write_in_line(
                    &mut lines,
                    &ans.text,
                    is_cur,
                    ans.editing_text,
                    inner.x,
                    row_y,
                    inner.width,
                );
            }
        }
        ElicitationFieldKind::MultiSelect { options, .. } => {
            for (idx, opt) in options.iter().enumerate() {
                raw_options.push((lines.len(), idx, false));
                let is_cur = !ans.editing_text && ans.cursor_opt == idx;
                let is_on = ans.checked.get(idx).copied().unwrap_or(false);
                let mark = if is_on { "[x]" } else { "[ ]" };
                let num = idx + 1;
                let desc = if opt.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", opt.description)
                };
                let label = format!(" {mark} {num}. {}{desc}", opt.label);
                let clipped = linebreak::clip(&label, inner.width);
                if is_cur {
                    let pad = max_w.saturating_sub(clipped.width());
                    lines.push(Line::styled(
                        format!("{clipped}{}", " ".repeat(pad)),
                        theme::picked_bar(),
                    ));
                } else {
                    let st = if is_on { theme::good() } else { theme::text() };
                    lines.push(Line::styled(clipped, st));
                }
            }
            if field.other_key.is_some() {
                raw_options.push((lines.len(), options.len(), true));
                let is_cur = ans.editing_text || ans.cursor_opt == options.len();
                let row_y = inner.y + lines.len() as u16;
                cursor_pos = push_write_in_line(
                    &mut lines,
                    &ans.text,
                    is_cur,
                    ans.editing_text,
                    inner.x,
                    row_y,
                    inner.width,
                );
            }
        }
        ElicitationFieldKind::Boolean { .. } => {
            for (idx, text) in [" [1] 是 (Yes)", " [2] 否 (No)"].iter().enumerate() {
                raw_options.push((lines.len(), idx, false));
                let is_cur = ans.cursor_opt == idx;
                let is_picked = (idx == 0) == ans.bool_val;
                let mark = if is_picked { "●" } else { "○" };
                let label = format!(" {mark}{text}");
                if is_cur {
                    let pad = max_w.saturating_sub(label.width());
                    lines.push(Line::styled(
                        format!("{label}{}", " ".repeat(pad)),
                        theme::picked_bar(),
                    ));
                } else {
                    lines.push(Line::styled(
                        label,
                        if is_picked { theme::good() } else { theme::text() },
                    ));
                }
            }
        }
        ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. } => {
            lines.push(Line::default());
            let row_y = inner.y + lines.len() as u16;
            cursor_pos = push_text_input_line(&mut lines, &ans.text, inner.x, row_y, inner.width);
        }
    }

    // 4. 若当前行超出可见高度，围绕激活行滚动窗口，并换算可见热区屏幕坐标
    let visible_h = usize::from(inner.height);
    let scroll_off = if lines.len() > visible_h {
        let focus_line = lines.len().saturating_sub(1);
        focus_line.saturating_sub(visible_h.saturating_sub(1))
    } else {
        0
    };
    if scroll_off == 0 && !raw_tabs.is_empty() {
        for (tx, tw, idx) in raw_tabs {
            hits.tabs.push((Rect::new(tx, inner.y, tw, 1), idx));
        }
    }
    for (line_idx, opt_idx, is_other) in raw_options {
        if line_idx >= scroll_off && line_idx < scroll_off + visible_h {
            let sy = inner.y + (line_idx - scroll_off) as u16;
            hits.options
                .push((Rect::new(inner.x, sy, inner.width, 1), opt_idx, is_other));
        }
    }
    let rendered: Vec<Line<'static>> = lines.into_iter().skip(scroll_off).take(visible_h).collect();
    frame.render_widget(Paragraph::new(rendered), inner);

    if let Some((cx, cy)) = cursor_pos {
        let adj_y = cy.saturating_sub(scroll_off as u16);
        if adj_y >= inner.y && adj_y < inner.bottom() {
            frame.set_cursor_position((cx.min(inner.right().saturating_sub(1)), adj_y));
        }
    }
    hits
}

fn push_write_in_line(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    highlighted: bool,
    editing: bool,
    base_x: u16,
    row_y: u16,
    width: u16,
) -> Option<(u16, u16)> {
    let prefix = " 自定义: ";
    let prefix_w = prefix.width();
    let room = usize::from(width).saturating_sub(prefix_w + 1).max(4);
    let shown = tail_slice(text, room);
    let display = if shown.is_empty() && !editing {
        "(按 i 或回车输入自定义回复)".to_string()
    } else {
        shown.clone()
    };

    if highlighted && !editing {
        let full = format!("{prefix}{display}");
        let clipped = linebreak::clip(&full, width);
        let pad = usize::from(width).saturating_sub(clipped.width());
        lines.push(Line::styled(
            format!("{clipped}{}", " ".repeat(pad)),
            theme::picked_bar(),
        ));
        return None;
    }

    lines.push(Line::from(vec![
        Span::styled(prefix, if editing { theme::accent_bold() } else { theme::dim() }),
        Span::styled(
            display,
            if shown.is_empty() && !editing {
                theme::faint()
            } else {
                theme::text()
            },
        ),
    ]));
    editing.then_some((base_x + prefix_w as u16 + shown.width() as u16, row_y))
}

fn push_text_input_line(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    base_x: u16,
    row_y: u16,
    width: u16,
) -> Option<(u16, u16)> {
    let prefix = " ❯ ";
    let prefix_w = prefix.width();
    let room = usize::from(width).saturating_sub(prefix_w + 1).max(4);
    let shown = tail_slice(text, room);
    lines.push(Line::from(vec![
        Span::styled(prefix, theme::accent_bold()),
        Span::styled(shown.clone(), theme::text()),
    ]));
    Some((base_x + prefix_w as u16 + shown.width() as u16, row_y))
}

fn tail_slice(s: &str, max_w: usize) -> String {
    if s.width() <= max_w {
        return s.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 0usize;
    for ch in s.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > max_w {
            break;
        }
        used += w;
        kept.push(ch);
    }
    kept.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use crate::acp::{
        AcpEvent, ElicitationField, ElicitationFieldKind, ElicitationOption, ElicitationRequest,
        SessionUpdate,
    };
    use crate::app::App;
    use crate::theme::{self, ThemePreset};

    fn buffer_to_text(buf: &ratatui::buffer::Buffer, w: u16, h: u16) -> String {
        let mut rendered = String::new();
        for y in 0..h {
            let mut x = 0;
            while x < w {
                let sym = buf[(x, y)].symbol();
                rendered.push_str(sym);
                x += u16::try_from(sym.width()).unwrap_or(1).max(1);
            }
            rendered.push('\n');
        }
        rendered
    }

    #[test]
    fn elicitation_drawer_handles_single_write_in_and_multi_select_without_blocking() {
        theme::init(ThemePreset::MiyuLavender, true);
        let mut app = App::new(None, r"D:\Code\agy-tui".into(), None, true, true);
        app.connecting = false;
        app.busy = true;

        // 1. ask_question 工具进入进行中时，绝不能被误标为后台运行任务
        app.handle_acp_event(AcpEvent::Update {
            session_id: "demo-8a9f2c11".into(),
            update: SessionUpdate::ToolCall {
                call_id: "ask_1".into(),
                name: "ask_question".into(),
                title: "Confirm deployment target".into(),
                kind: "other".into(),
                status: "in_progress".into(),
                raw_input: None,
                output: None,
                diffs: Vec::new(),
            },
        });
        assert!(!app.has_background_tasks());

        // 2. 收到 elicitation/create 请求后渲染提问抽屉并支持单选 Write-in + 多选勾选 + 组装校验
        app.handle_acp_event(AcpEvent::ElicitationRequested(ElicitationRequest {
            rpc_id: serde_json::json!(42),
            session_id: "demo-8a9f2c11".into(),
            elicitation_id: String::new(),
            mode: "form".into(),
            message: "The agent has 2 questions.".into(),
            url: None,
            fields: vec![
                ElicitationField {
                    key: "q0".into(),
                    title: "Which target?".into(),
                    description: String::new(),
                    required: true,
                    kind: ElicitationFieldKind::SingleSelect(vec![
                        ElicitationOption {
                            value: "prod".into(),
                            label: "Production".into(),
                            description: String::new(),
                        },
                        ElicitationOption {
                            value: "Other".into(),
                            label: "Other".into(),
                            description: String::new(),
                        },
                    ]),
                    other_key: Some("q0_other".into()),
                    other_title: "Custom".into(),
                },
                ElicitationField {
                    key: "q1".into(),
                    title: "Select checks".into(),
                    description: String::new(),
                    required: true,
                    kind: ElicitationFieldKind::MultiSelect {
                        options: vec![
                            ElicitationOption {
                                value: "unit".into(),
                                label: "Unit tests".into(),
                                description: String::new(),
                            },
                            ElicitationOption {
                                value: "lint".into(),
                                label: "Clippy".into(),
                                description: String::new(),
                            },
                        ],
                        min_items: Some(1),
                        max_items: None,
                    },
                    other_key: None,
                    other_title: String::new(),
                },
            ],
        }));

        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let drawer_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
        assert!(
            drawer_view.contains("代理提问 (1/2)") && drawer_view.contains("Which target?"),
            "expected elicitation drawer:\n{drawer_view}"
        );

        // 按 2 选中 Other（因伴随 q0_other 且为空，自动进入自定义输入），键入 staging 后按 Enter 推进到第 2 题
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('2'),
            crossterm::event::KeyModifiers::NONE,
        ));
        for ch in "staging".chars() {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(ch),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.elicitation.as_ref().unwrap().active_field, 1);

        // 第 2 题空格勾选第 1 项并按方向下+空格勾选第 2 项
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        let built = app.elicitation.as_ref().unwrap().build_content().unwrap();
        assert_eq!(
            built,
            serde_json::json!({
                "q0": "Other",
                "q0_other": "staging",
                "q1": ["unit", "lint"]
            })
        );

        // Esc 取消并关闭提问抽屉
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.elicitation.is_none());
    }
}
