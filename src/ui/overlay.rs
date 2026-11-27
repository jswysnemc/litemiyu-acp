//! 模态选择器（模型/思考等级/模式/历史会话/主题/状态/队列/帮助）状态与渲染模块。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::linebreak;
use crate::theme;
use crate::transcript::short_id;
pub use super::format::{
    format_compact_time, format_relative_time, parse_timestamp_secs, same_directory,
    shorten_path_left,
};
pub use super::overlay_drawers::{draw_completion_menu, draw_permission_drawer};
use super::HoverTarget;
use super::overlay_hits::{OverlayFooterAction, PickerHits, build_right_hint_line};

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
    Queue,
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

pub fn draw_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &PickerState,
    hover: &HoverTarget,
) -> PickerHits {
    if picker.kind == PickerKind::Resume {
        return draw_resume_picker(frame, area, picker, hover);
    }

    let w = area.width.saturating_sub(6).clamp(36, 82);
    let filtered = picker.filtered();
    let body_rows = filtered.len().clamp(3, 14) as u16;
    let h = (body_rows + 5).min(area.height.saturating_sub(4)).max(7);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);
    let mut hits = PickerHits {
        popup,
        ..PickerHits::default()
    };

    let p = theme::palette();
    frame.render_widget(Clear, popup);
    let title_line = Line::from(vec![Span::styled(
        format!(" {} ", picker.title),
        theme::accent_bold(),
    )]);
    let bottom_y = popup.bottom().saturating_sub(1);
    let (hint_line, footer_actions) = if picker.kind == PickerKind::Help {
        build_right_hint_line(
            popup,
            bottom_y,
            &[
                ("↑↓/滚轮 浏览 · ", None),
                ("Esc/点外部 关闭", Some(OverlayFooterAction::Close)),
            ],
            hover,
        )
    } else {
        build_right_hint_line(
            popup,
            bottom_y,
            &[
                ("↑↓/点击 选择 · ", None),
                ("Enter 确认", Some(OverlayFooterAction::Confirm)),
                (" · ", None),
                ("Esc/点外部 关闭", Some(OverlayFooterAction::Close)),
            ],
            hover,
        )
    };
    hits.footer_actions = footer_actions;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title_top(title_line)
        .title_bottom(hint_line)
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 2 {
        return hits;
    }

    // 搜索栏（Help / Status / Queue 不需要搜索栏）
    let no_search = matches!(
        picker.kind,
        PickerKind::Help | PickerKind::Status | PickerKind::Queue
    );
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
        return hits;
    }
    let sel = picker.selected.min(filtered.len().saturating_sub(1));
    let start = if sel >= visible { sel + 1 - visible } else { 0 };
    let max_w = usize::from(list_area.width);

    let mut lines = Vec::new();
    if filtered.is_empty() {
        lines.push(Line::styled("  无匹配项", theme::dim()));
    } else {
        for (row_offset, (i, (_, item))) in filtered.iter().enumerate().skip(start).take(visible).enumerate() {
            if picker.kind != PickerKind::Help {
                hits.items.push((
                    Rect::new(list_area.x, list_area.y + row_offset as u16, list_area.width, 1),
                    i,
                ));
            }
            let is_sel = i == sel && picker.kind != PickerKind::Help;
            let mark = if item.active { "● " } else { "  " };
            let swatches = if picker.kind == PickerKind::Theme {
                theme::swatches_for_id(&item.id)
            } else {
                None
            };
            // 主题选择器在右侧绘制 5 色代表色块（宽屏留间隔 11 列，窄屏紧凑 7 列）
            let (swatch_w, spaced_swatches) = if swatches.is_some() && max_w >= 44 {
                (11usize, true)
            } else if swatches.is_some() && max_w >= 26 {
                (7usize, false)
            } else {
                (0usize, false)
            };
            let text_max_w = max_w.saturating_sub(swatch_w);
            let left = format!(" {mark}{}", item.label);
            let detail_room = text_max_w.saturating_sub(left.width() + 3);
            let detail = if detail_room >= 6 && !item.detail.is_empty() {
                format!("  {}", linebreak::clip(&item.detail, detail_room as u16))
            } else {
                String::new()
            };
            let full = format!("{left}{detail}");
            let clipped = linebreak::clip(&full, text_max_w as u16);
            let pad_to_swatch = text_max_w.saturating_sub(clipped.width());
            if is_sel {
                let bar = theme::picked_bar();
                let mut spans = vec![Span::styled(
                    format!("{clipped}{}", " ".repeat(pad_to_swatch)),
                    bar,
                )];
                if let Some(colors) = swatches.filter(|_| swatch_w > 0) {
                    let swatch_bg = bar.bg.unwrap_or(theme::palette().accent);
                    spans.push(Span::styled(" ", Style::new().bg(swatch_bg)));
                    for (idx_c, color) in colors.into_iter().enumerate() {
                        if spaced_swatches && idx_c > 0 {
                            spans.push(Span::styled(" ", Style::new().bg(swatch_bg)));
                        }
                        spans.push(Span::styled("■", Style::new().fg(color).bg(swatch_bg)));
                    }
                    spans.push(Span::styled(" ", Style::new().bg(swatch_bg)));
                }
                lines.push(Line::from(spans));
            } else {
                let clipped_label = linebreak::clip(&item.label, text_max_w.saturating_sub(3) as u16);
                let used_w = 1 + mark.width() + clipped_label.width() + detail.width();
                let mut spans = vec![
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
                        clipped_label,
                        if item.active {
                            theme::accent_bold()
                        } else {
                            theme::text()
                        },
                    ),
                    Span::styled(detail, theme::dim()),
                ];
                if let Some(colors) = swatches.filter(|_| swatch_w > 0) {
                    let gap = text_max_w.saturating_sub(used_w) + 1;
                    spans.push(Span::styled(" ".repeat(gap), Style::new()));
                    for (idx_c, color) in colors.into_iter().enumerate() {
                        if spaced_swatches && idx_c > 0 {
                            spans.push(Span::styled(" ", Style::new()));
                        }
                        spans.push(Span::styled("■", Style::new().fg(color)));
                    }
                    spans.push(Span::styled(" ", Style::new()));
                }
                lines.push(Line::from(spans));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    hits
}

/// 专属 `/resume` 会话恢复面板：双行结构化卡片、当前目录/全部范围 Tab 切换、最近时间右对齐、路径尾部保留。
fn draw_resume_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &PickerState,
    hover: &HoverTarget,
) -> PickerHits {
    let w = area.width.saturating_sub(4).clamp(46, 96);
    let filtered = picker.filtered();
    let card_rows = filtered.len().clamp(3, 8) as u16 * 2;
    let h = (card_rows + 6)
        .min(area.height.saturating_sub(4))
        .max(12);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);
    let mut hits = PickerHits {
        popup,
        ..PickerHits::default()
    };

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
    let bottom_y = popup.bottom().saturating_sub(1);
    let (hint_line, footer_actions) = build_right_hint_line(
        popup,
        bottom_y,
        &[
            ("↑↓/点击 恢复 · ", None),
            ("Tab 切换范围", Some(OverlayFooterAction::ToggleResumeScope)),
            (" · ", None),
            ("Enter 确认", Some(OverlayFooterAction::Confirm)),
            (" · ", None),
            ("Esc/点外部 关闭", Some(OverlayFooterAction::Close)),
        ],
        hover,
    );
    hits.footer_actions = footer_actions;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title_top(title_line)
        .title_top(Line::styled(pos_label, theme::dim()).right_aligned())
        .title_bottom(hint_line)
        .style(Style::new().bg(p.panel_bg).fg(p.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.height < 5 || inner.width < 20 {
        return hits;
    }

    // 1. 顶部范围标签栏：当前目录 vs 全部会话（支持鼠标单击切换范围）
    let tab_cur = format!(" 当前目录 ({cur_dir_count}) ");
    let tab_all = format!(" 全部会话 ({total_count}) ");
    let cur_w = u16::try_from(tab_cur.width()).unwrap_or(0);
    let all_w = u16::try_from(tab_all.width()).unwrap_or(0);
    let cur_x = inner.x.saturating_add(1);
    let all_x = cur_x.saturating_add(cur_w).saturating_add(1);
    if cur_x < inner.right() && cur_w > 0 {
        hits.resume_tabs.push((
            Rect::new(cur_x, inner.y, cur_w.min(inner.right().saturating_sub(cur_x)), 1),
            ResumeScope::CurrentDir,
        ));
    }
    if all_x < inner.right() && all_w > 0 {
        hits.resume_tabs.push((
            Rect::new(all_x, inner.y, all_w.min(inner.right().saturating_sub(all_x)), 1),
            ResumeScope::All,
        ));
    }
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
        return hits;
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
        return hits;
    }

    let start = if sel >= visible_cards {
        sel + 1 - visible_cards
    } else {
        0
    };
    let max_w = usize::from(list_area.width);

    for (card_offset, (i, (_, item))) in filtered
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_cards)
        .enumerate()
    {
        hits.items.push((
            Rect::new(list_area.x, list_area.y + (card_offset * 2) as u16, list_area.width, 2),
            i,
        ));
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
    hits
}
