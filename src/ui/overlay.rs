//! 悬浮补全菜单、模态选择器（模型/思考等级/模式/历史会话/主题/帮助）与权限审批抽屉。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::acp::PermissionRequest;
use crate::linebreak;
use crate::theme;
use crate::transcript::short_id;
pub use super::format::{
    format_compact_time, format_relative_time, parse_timestamp_secs, same_directory,
    shorten_path_left,
};

#[derive(Debug, Clone)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub detail: String,
    /// 右侧附加元信息（如最近时间 `3分钟前`）
    pub meta: String,
    /// 是否属于当前工作目录（供 `/resume` 筛选与徽标展示）
    pub same_cwd: bool,
    pub active: bool,
}

impl PickerItem {
    pub fn simple(
        id: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
        active: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
            meta: String::new(),
            same_cwd: false,
            active,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Effort,
    Mode,
    Resume,
    Theme,
    Status,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeScope {
    CurrentDir,
    All,
}

#[derive(Debug, Clone)]
pub struct PickerState {
    pub kind: PickerKind,
    pub title: String,
    pub query: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
    pub scope: ResumeScope,
    pub current_cwd: String,
}

impl PickerState {
    pub fn new(
        kind: PickerKind,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        selected: usize,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            query: String::new(),
            items,
            selected,
            scope: ResumeScope::All,
            current_cwd: String::new(),
        }
    }

    pub fn filtered(&self) -> Vec<(usize, &PickerItem)> {
        let q = self.query.trim().to_ascii_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if self.kind == PickerKind::Resume
                    && self.scope == ResumeScope::CurrentDir
                    && !item.same_cwd
                {
                    return false;
                }
                q.is_empty()
                    || item.label.to_ascii_lowercase().contains(&q)
                    || item.detail.to_ascii_lowercase().contains(&q)
                    || item.id.to_ascii_lowercase().contains(&q)
                    || item.meta.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn current_dir_count(&self) -> usize {
        self.items.iter().filter(|i| i.same_cwd).count()
    }

    pub fn toggle_scope(&mut self) {
        if self.kind != PickerKind::Resume {
            return;
        }
        self.scope = match self.scope {
            ResumeScope::CurrentDir => ResumeScope::All,
            ResumeScope::All => ResumeScope::CurrentDir,
        };
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            len - 1
        } else {
            self.selected - 1
        };
    }

    pub fn move_down(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
    }

    pub fn page_up(&mut self, step: usize) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        self.selected = self.selected.saturating_sub(step);
    }

    pub fn page_down(&mut self, step: usize) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + step).min(len - 1);
    }

    pub fn selected_item(&self) -> Option<&PickerItem> {
        let list = self.filtered();
        list.get(self.selected)
            .or_else(|| list.first())
            .map(|(_, item)| *item)
    }
}

use crate::completion::{CompletionKind, CompletionMenu};

/// 绘制输入框上方的统一悬浮补全菜单（支持 `/` 命令、`#` Skills 与 `@` 工作区文件）。
///
/// 参数:
/// - `frame`: 当前渲染帧
/// - `input_frame`: 输入框外框区域
/// - `menu`: 补全菜单快照
/// - `selected`: 当前选中项索引
pub fn draw_completion_menu(
    frame: &mut Frame,
    input_frame: Rect,
    menu: &CompletionMenu,
    selected: usize,
) {
    let suggestions = &menu.items;
    if suggestions.is_empty() || input_frame.y < 3 {
        return;
    }
    let visible = suggestions.len().min(8);
    let height = visible as u16 + 2;
    let y = input_frame.y.saturating_sub(height);
    let area = Rect::new(input_frame.x, y, input_frame.width, height);

    let (title, hint) = match menu.kind {
        CompletionKind::Command => (
            format!(" 命令 ({}) ", suggestions.len()),
            " ↑↓ 选择 · Tab 补全 · Enter 执行 · Esc 关闭 ",
        ),
        CompletionKind::Skill => (
            format!(" Skills ({}) ", suggestions.len()),
            " ↑↓ 选择 · Tab/Enter 插入 · Esc 关闭 ",
        ),
        CompletionKind::File => (
            format!(" 工作区文件 ({}) ", suggestions.len()),
            " ↑↓ 选择 · Tab/Enter 插入 · Esc 关闭 ",
        ),
    };

    let p = theme::palette();
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::faint())
        .title_top(Line::styled(title, theme::accent()).left_aligned())
        .title_top(Line::styled(hint, theme::faint()).right_aligned())
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let start = if selected >= visible {
        selected + 1 - visible
    } else {
        0
    };
    let mut lines = Vec::new();
    let max_w = usize::from(inner.width);
    let label_cap = match menu.kind {
        CompletionKind::File => 28,
        CompletionKind::Command | CompletionKind::Skill => 24,
    };
    for (idx, item) in suggestions.iter().enumerate().skip(start).take(visible) {
        let is_sel = idx == selected;
        let clipped_label = linebreak::clip(&item.label, label_cap);
        let label_w = clipped_label.width().max(16);
        let pad_after_label = label_w.saturating_sub(clipped_label.width());
        let desc_room = max_w.saturating_sub(label_w + 4);
        let desc = linebreak::clip(&item.description, desc_room as u16);
        let row_text = format!(" {}{}  {}", clipped_label, " ".repeat(pad_after_label), desc);

        if is_sel {
            let pad = max_w.saturating_sub(row_text.width());
            lines.push(Line::styled(
                format!("{row_text}{}", " ".repeat(pad)),
                theme::picked_bar(),
            ));
        } else {
            let label_style = match menu.kind {
                CompletionKind::File => theme::md_url(),
                CompletionKind::Skill => theme::picked(),
                CompletionKind::Command => {
                    if item.accent_primary {
                        theme::accent()
                    } else {
                        theme::picked()
                    }
                }
            };
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(clipped_label, label_style),
                Span::raw(" ".repeat(pad_after_label + 2)),
                Span::styled(desc, theme::dim()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn draw_picker(frame: &mut Frame, area: Rect, picker: &PickerState) {
    if picker.kind == PickerKind::Resume {
        draw_resume_picker(frame, area, picker);
        return;
    }

    let w = area.width.saturating_sub(6).clamp(36, 82);
    let filtered = picker.filtered();
    let body_rows = filtered.len().clamp(3, 14) as u16;
    let h = (body_rows + 5).min(area.height.saturating_sub(4)).max(7);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    let p = theme::palette();
    frame.render_widget(Clear, popup);
    let title_line = Line::from(vec![Span::styled(
        format!(" {} ", picker.title),
        theme::accent_bold(),
    )]);
    let hint_line = Line::from(vec![Span::styled(
        " ↑↓ 选择 · Enter 确认 · Esc 关闭 ",
        theme::faint(),
    )]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title_top(title_line)
        .title_bottom(hint_line.right_aligned())
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 {
        return;
    }

    // 搜索栏（Help / Status 不需要搜索栏）
    let no_search = matches!(picker.kind, PickerKind::Help | PickerKind::Status);
    let list_area = if !no_search {
        let search_line = if picker.query.is_empty() {
            Line::from(vec![
                Span::styled(" ❯ ", theme::accent()),
                Span::styled("输入关键字筛选…", theme::faint()),
            ])
        } else {
            Line::from(vec![
                Span::styled(" ❯ ", theme::accent()),
                Span::styled(picker.query.clone(), theme::text()),
            ])
        };
        frame.render_widget(
            Paragraph::new(search_line),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                "─".repeat(usize::from(inner.width)),
                theme::faint(),
            )),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        )
    } else {
        inner
    };

    let visible = usize::from(list_area.height);
    if visible == 0 {
        return;
    }
    let sel = picker.selected.min(filtered.len().saturating_sub(1));
    let start = if sel >= visible { sel + 1 - visible } else { 0 };
    let max_w = usize::from(list_area.width);

    let mut lines = Vec::new();
    if filtered.is_empty() {
        lines.push(Line::styled("  无匹配项", theme::dim()));
    } else {
        for (i, (_, item)) in filtered.iter().enumerate().skip(start).take(visible) {
            let is_sel = i == sel && picker.kind != PickerKind::Help;
            let mark = if item.active { "● " } else { "  " };
            let left = format!(" {mark}{}", item.label);
            let detail_room = max_w.saturating_sub(left.width() + 3);
            let detail = if detail_room >= 6 && !item.detail.is_empty() {
                format!("  {}", linebreak::clip(&item.detail, detail_room as u16))
            } else {
                String::new()
            };
            let full = format!("{left}{detail}");
            let clipped = linebreak::clip(&full, list_area.width);
            if is_sel {
                let pad = max_w.saturating_sub(clipped.width());
                lines.push(Line::styled(
                    format!("{clipped}{}", " ".repeat(pad)),
                    theme::picked_bar(),
                ));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(" ", Style::new()),
                    Span::styled(
                        mark,
                        if item.active {
                            theme::good()
                        } else {
                            theme::faint()
                        },
                    ),
                    Span::styled(
                        linebreak::clip(&item.label, list_area.width.saturating_sub(3)),
                        if item.active {
                            theme::accent_bold()
                        } else {
                            theme::text()
                        },
                    ),
                    Span::styled(detail, theme::dim()),
                ]));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), list_area);
}

/// 专属 `/resume` 会话恢复面板：双行结构化卡片、当前目录/全部范围 Tab 切换、最近时间右对齐、路径尾部保留。
fn draw_resume_picker(frame: &mut Frame, area: Rect, picker: &PickerState) {
    let w = area.width.saturating_sub(4).clamp(46, 96);
    let filtered = picker.filtered();
    let card_rows = filtered.len().clamp(3, 8) as u16 * 2;
    let h = (card_rows + 6)
        .min(area.height.saturating_sub(4))
        .max(12);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    let p = theme::palette();
    frame.render_widget(Clear, popup);

    let cur_dir_count = picker.current_dir_count();
    let total_count = picker.items.len();
    let sel = picker.selected.min(filtered.len().saturating_sub(1));

    let title_line = Line::from(vec![Span::styled(" 恢复历史会话 ", theme::accent_bold())]);
    let pos_label = if filtered.is_empty() {
        " 0 条 ".to_string()
    } else {
        format!(" {}/{} ", sel + 1, filtered.len())
    };
    let hint_line = Line::from(vec![Span::styled(
        " ↑↓ 选择 · Tab 切换目录范围 · Enter 恢复 · Esc 关闭 ",
        theme::faint(),
    )]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title_top(title_line)
        .title_top(Line::styled(pos_label, theme::dim()).right_aligned())
        .title_bottom(hint_line.right_aligned())
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 5 || inner.width < 20 {
        return;
    }

    // 1. 顶部范围标签栏：当前目录 vs 全部会话
    let tab_cur = format!(" 当前目录 ({cur_dir_count}) ");
    let tab_all = format!(" 全部会话 ({total_count}) ");
    let (cur_style, all_style) = match picker.scope {
        ResumeScope::CurrentDir => (theme::picked_bar(), theme::dim()),
        ResumeScope::All => (theme::dim(), theme::picked_bar()),
    };
    let cwd_room = inner
        .width
        .saturating_sub((tab_cur.width() + tab_all.width() + 6) as u16);
    let cwd_preview = if !picker.current_cwd.is_empty() && cwd_room >= 10 {
        format!(
            "  {}",
            shorten_path_left(&picker.current_cwd, cwd_room.saturating_sub(2))
        )
    } else {
        String::new()
    };
    let tab_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(tab_cur, cur_style),
        Span::raw(" "),
        Span::styled(tab_all, all_style),
        Span::styled(cwd_preview, theme::faint()),
    ]);
    frame.render_widget(
        Paragraph::new(tab_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // 2. 搜索栏
    let search_line = if picker.query.is_empty() {
        Line::from(vec![
            Span::styled(" ❯ ", theme::accent()),
            Span::styled("输入关键字筛选（标题 / 目录 / ID）…", theme::faint()),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ❯ ", theme::accent()),
            Span::styled(picker.query.clone(), theme::text()),
        ])
    };
    frame.render_widget(
        Paragraph::new(search_line),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // 3. 分隔线
    frame.render_widget(
        Paragraph::new(Line::styled(
            "─".repeat(usize::from(inner.width)),
            theme::faint(),
        )),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );

    // 4. 双行结构化会话卡片列表
    let list_area = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(3),
    );
    let visible_cards = usize::from(list_area.height) / 2;
    if visible_cards == 0 {
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    if filtered.is_empty() {
        let empty_msg = if picker.scope == ResumeScope::CurrentDir && total_count > 0 {
            format!("  当前目录暂无历史会话 · 按 Tab 查看全部 {total_count} 条会话")
        } else if !picker.query.is_empty() {
            "  无匹配的会话（可清空关键字或按 Tab 切换范围）".to_string()
        } else {
            "  暂无可恢复的历史会话".to_string()
        };
        lines.push(Line::default());
        lines.push(Line::styled(
            linebreak::clip(&empty_msg, list_area.width),
            theme::dim(),
        ));
        frame.render_widget(Paragraph::new(lines), list_area);
        return;
    }

    let start = if sel >= visible_cards {
        sel + 1 - visible_cards
    } else {
        0
    };
    let max_w = usize::from(list_area.width);

    for (i, (_, item)) in filtered.iter().enumerate().skip(start).take(visible_cards) {
        let is_sel = i == sel;
        let row_bg = if is_sel {
            Some(p.row_focus_bg)
        } else {
            None
        };
        let with_bg = |st: Style| match row_bg {
            Some(bg) => st.bg(bg),
            None => st,
        };

        let bar = if is_sel { "▎ " } else { "  " };
        let bar_style = if is_sel {
            with_bg(theme::accent_bold())
        } else {
            with_bg(theme::faint())
        };
        let dot = if item.active { "● " } else { "  " };
        let dot_style = if item.active {
            with_bg(theme::good())
        } else {
            with_bg(theme::faint())
        };

        // 第 1 行：左 [标题] + 右 [最近时间]
        let time_str = if item.meta.is_empty() {
            String::new()
        } else {
            format!(" {} ", item.meta)
        };
        let time_w = time_str.width();
        let title_room = max_w.saturating_sub(4 + time_w + 1).max(6);
        let title_clipped = linebreak::clip(&item.label, title_room as u16);
        let title_style = if is_sel || item.active {
            with_bg(Style::new().fg(p.text).add_modifier(Modifier::BOLD))
        } else {
            with_bg(theme::text())
        };
        let time_style = if is_sel {
            with_bg(theme::accent())
        } else {
            with_bg(theme::dim())
        };
        let line1_used = 4 + title_clipped.width() + time_w;
        let line1_gap = max_w.saturating_sub(line1_used);

        lines.push(Line::from(vec![
            Span::styled(bar, bar_style),
            Span::styled(dot, dot_style),
            Span::styled(title_clipped, title_style),
            Span::styled(" ".repeat(line1_gap), with_bg(Style::new())),
            Span::styled(time_str, time_style),
        ]));

        // 第 2 行：左 [#id · [当前目录] · 精简路径] + 右 [当前会话]
        let id_str = format!("#{}", short_id(&item.id));
        let cwd_tag = if item.same_cwd { " [当前目录]" } else { "" };
        let active_tag = if item.active { " 当前会话 " } else { "" };
        let fixed_w = 4 + id_str.width() + cwd_tag.width() + active_tag.width() + 4;
        let path_room = max_w.saturating_sub(fixed_w);
        let path_str = if !item.detail.is_empty() && path_room >= 6 {
            format!(" · {}", shorten_path_left(&item.detail, path_room as u16))
        } else {
            String::new()
        };

        let line2_used =
            4 + id_str.width() + cwd_tag.width() + path_str.width() + active_tag.width();
        let line2_gap = max_w.saturating_sub(line2_used);

        let mut row2 = vec![
            Span::styled(bar, bar_style),
            Span::styled("  ", with_bg(Style::new())),
            Span::styled(id_str, with_bg(theme::faint())),
        ];
        if !cwd_tag.is_empty() {
            row2.push(Span::styled(cwd_tag, with_bg(theme::good())));
        }
        if !path_str.is_empty() {
            row2.push(Span::styled(path_str, with_bg(theme::dim())));
        }
        row2.push(Span::styled(" ".repeat(line2_gap), with_bg(Style::new())));
        if !active_tag.is_empty() {
            row2.push(Span::styled(active_tag, with_bg(theme::good())));
        }
        lines.push(Line::from(row2));
    }

    frame.render_widget(Paragraph::new(lines), list_area);
}

pub fn draw_permission_drawer(
    frame: &mut Frame,
    area: Rect,
    req: &PermissionRequest,
    selected: usize,
) {
    let p = theme::palette();
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::warn())
        .title_top(Line::styled(" ◈ 权限确认 ", theme::warn()))
        .title_bottom(
            Line::styled(" ↑↓ 选择 · 1-9/Enter 确认 · Esc 拒绝 ", theme::faint()).right_aligned(),
        )
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("工具: ", theme::dim()),
        Span::styled(
            linebreak::clip(&req.title, inner.width.saturating_sub(6)),
            theme::accent_bold(),
        ),
    ]));

    if let Some(raw) = &req.raw_input {
        if let Some(cmd) = raw["CommandLine"]
            .as_str()
            .or_else(|| raw["command"].as_str())
        {
            let one_line = cmd.replace('\n', " ");
            let clipped = linebreak::clip(&one_line, inner.width.saturating_sub(2));
            let mut spans = vec![Span::styled("$ ", theme::picked())];
            for piece in crate::markdown::code::highlight_shell_line(&clipped, None) {
                spans.push(Span::styled(piece.text, piece.style));
            }
            lines.push(Line::from(spans));
        } else {
            let preview = raw["Path"]
                .as_str()
                .or_else(|| raw["path"].as_str())
                .map(String::from)
                .unwrap_or_else(|| raw.to_string());
            lines.push(Line::styled(
                linebreak::clip(&preview.replace('\n', " "), inner.width),
                theme::md_code(),
            ));
        }
    }
    lines.push(Line::default());

    let max_w = usize::from(inner.width);
    for (idx, opt) in req.options.iter().enumerate() {
        let is_sel = idx == selected;
        let label = format!(" [{}] {}", idx + 1, opt.name);
        if is_sel {
            let pad = max_w.saturating_sub(label.width());
            lines.push(Line::styled(
                format!("{label}{}", " ".repeat(pad)),
                theme::picked_bar(),
            ));
        } else {
            let style = if opt.kind.contains("reject") {
                theme::error()
            } else if opt.kind.contains("allow") {
                theme::good()
            } else {
                theme::text()
            };
            lines.push(Line::styled(label, style));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
