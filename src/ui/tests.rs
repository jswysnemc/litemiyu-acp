//! UI 端到端渲染、交互、划词复制与后台任务集成测试。

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

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
fn demo_screen_renders_cleanly_at_wide_and_narrow_sizes() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    app.auto_follow = false;
    app.scroll = 0;

    for (w, h) in [(140, 46), (84, 34)] {
        app.auto_follow = false;
        app.scroll = 0;
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let top_view = buffer_to_text(terminal.backend().buffer(), w, h);
        assert!(top_view.contains("修复方案输出"), "missing heading at {w}x{h}");
        assert!(top_view.contains("┌"), "missing boxed table at {w}x{h}");
        if w >= 115 {
            assert!(top_view.contains("工作区"), "missing sidebar at {w}x{h}");
            assert!(top_view.contains("▰"), "missing context bar fill at {w}x{h}");
            assert!(
                top_view.contains("3分前") && top_view.contains("昨天"),
                "sidebar recent sessions should show compact relative time at {w}x{h}:\n{top_view}"
            );
        }

        // 滚到底部验证新版 sai 风格代码块、右下角当前目录与 LiteMiyu 收尾元信息行
        app.auto_follow = true;
        terminal.draw(|f| app.render(f)).unwrap();
        let bottom_view = buffer_to_text(terminal.backend().buffer(), w, h);
        assert!(bottom_view.contains("kotlin"), "missing code language tag at {w}x{h}:\n{bottom_view}");
        assert!(
            !bottom_view.contains("╭─ code") && !bottom_view.contains('▎'),
            "code block should have no decorative borders/rails"
        );
        assert!(bottom_view.contains("▣"), "missing done meta row at {w}x{h}");
        let footer_row = bottom_view.lines().last().unwrap_or("");
        assert!(
            footer_row.contains("litemiyu-acp"),
            "bottom-right footer should show current working directory at {w}x{h}: {footer_row:?}"
        );

        // 验证框选整块代码块时自动忽略语言标签与左侧边距，只复制纯净代码（保留代码自身缩进）
        if w == 140 {
            let bz = app.last_hits.body_text;
            let code_header_y = app
                .last_hits
                .row_copy_meta
                .iter()
                .find(|(y, m)| {
                    m.decorative
                        && app
                            .last_hits
                            .screen_snapshot
                            .slice_row_text(**y, bz.x, bz.right())
                            .trim()
                            == "kotlin"
                })
                .map(|(y, _)| *y)
                .expect("find kotlin code header row");
            let code_sel = crate::ui::TextSelection {
                anchor: (bz.x, code_header_y),
                head: (bz.right() - 1, code_header_y + 7),
                zone: bz,
            };
            let copied_code = app.last_hits.extract_selection(code_sel);
            assert!(
                !copied_code.contains("kotlin\n") && !copied_code.starts_with("kotlin"),
                "code copy must skip decorative language header:\n{copied_code}"
            );
            assert!(
                copied_code.starts_with("// OffToLockscreenTransitionViewModel.kt"),
                "code copy must strip decorative left margin without extra spaces:\n{copied_code:?}"
            );
            assert!(
                copied_code.contains("\n    transitionAnimation.sharedFlow("),
                "code copy must preserve code's own indentation:\n{copied_code:?}"
            );
        }

        // 验证正文区跨行划词提取：精准提取正文且绝不串入右侧边栏
        if w == 140 {
            let bz = app.last_hits.body_text;
            let sel = crate::ui::TextSelection {
                anchor: (bz.x, bz.y),
                head: (bz.right() - 1, bz.y + 8),
                zone: bz,
            };
            let picked = app.last_hits.extract_selection(sel);
            assert!(!picked.is_empty(), "selection should not be empty");
            assert!(!picked.contains("工作区"), "body selection must not bleed into sidebar: {picked}");

            // 验证点击滚动条、回到底部按钮与上下文进度条
            app.auto_follow = false;
            app.scroll = 0;
            terminal.draw(|f| app.render(f)).unwrap();
            let bar = app.last_hits.scrollbar;
            let btn = app.last_hits.bottom_button;
            let ctx = app.last_hits.sidebar.context_area;
            assert!(bar.width > 0 && bar.height > 0, "scrollbar hitbox should exist");
            assert!(btn.width > 0, "bottom button hitbox should exist when scrolled up");
            assert!(ctx.width > 0 && ctx.height > 0, "context bar hitbox should exist");

            // 1) 点击滚动条中段
            let mid_y = bar.y + bar.height / 2;
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: bar.x,
                row: mid_y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: bar.x,
                row: mid_y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            assert!(app.scroll > 0, "clicking scrollbar midpoint should scroll down");

            // 2) 点击回到底部按钮
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: btn.x + 1,
                row: btn.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: btn.x + 1,
                row: btn.y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            assert!(app.auto_follow, "clicking bottom button should restore auto_follow");

            // 3) 点击侧边栏上下文进度条弹出状态/压缩浮层
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: ctx.x + 2,
                row: ctx.bottom() - 1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: ctx.x + 2,
                row: ctx.bottom() - 1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            assert!(
                app.picker
                    .as_ref()
                    .is_some_and(|p| p.kind == crate::ui::overlay::PickerKind::Status),
                "clicking context progress bar should open status/compact picker"
            );
            app.picker = None;

            // 4) 验证读取与编辑工具均默认折叠（显示行号范围/增删统计），展开后呈现 sai 风格预览与 Diff
            app.auto_follow = false;
            app.scroll = 0;
            terminal.draw(|f| app.render(f)).unwrap();
            let collapsed_view = buffer_to_text(terminal.backend().buffer(), w, h);
            assert!(collapsed_view.contains("44–48"), "missing read range badge:\n{collapsed_view}");
            assert!(collapsed_view.contains("+1 -0"), "missing diff summary counts:\n{collapsed_view}");
            assert!(
                !collapsed_view.contains("44  val lockscreenAlpha"),
                "read preview should be collapsed by default:\n{collapsed_view}"
            );
            assert!(
                !collapsed_view.contains("+          onFinish = { 1f },"),
                "diff should be collapsed by default:\n{collapsed_view}"
            );

            // 点击读取行（按 diff 相同的点击打开逻辑）单独展开读取预览
            let read_screen_y = collapsed_view
                .lines()
                .enumerate()
                .find(|(_, l)| l.contains("读取 · OffToLockscreenTransitionViewModel.kt"))
                .map(|(y, _)| y as u16)
                .expect("find read tool header row");
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: bz.x + 2,
                row: read_screen_y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: bz.x + 2,
                row: read_screen_y,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
            terminal.draw(|f| app.render(f)).unwrap();
            let read_open_view = buffer_to_text(terminal.backend().buffer(), w, h);
            assert!(
                read_open_view.contains("44  val lockscreenAlpha"),
                "clicking read tool should open sai-style numbered code preview:\n{read_open_view}"
            );
            assert!(
                read_open_view.contains("• D:\\Code\\Systemui-dev")
                    || read_open_view.contains("• D:/Code/Systemui-dev"),
                "expanded read tool should show clickable file path like diff:\n{read_open_view}"
            );

            app.transcript.toggle_all_details(true);
            terminal.draw(|f| app.render(f)).unwrap();
            let diff_view = buffer_to_text(terminal.backend().buffer(), w, h);
            assert!(
                diff_view.contains("+          onFinish = { 1f },"),
                "missing rendered sai-style unified diff line after expand:\n{diff_view}"
            );
            app.transcript.toggle_all_details(false);
        }
    }
}

#[test]
fn chat_input_box_grows_dynamically_and_caps_at_max_rows() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();

    // 1 行时框高 = 3 (1 + 2 边框)
    app.editor.set_text("hello");
    terminal.draw(|f| app.render(f)).unwrap();
    assert_eq!(app.last_hits.input_area.height, 3);

    // 4 行时自动增高到 6 (4 + 2 边框)
    app.editor.set_text("line1\nline2\nline3\nline4");
    terminal.draw(|f| app.render(f)).unwrap();
    assert_eq!(app.last_hits.input_area.height, 6);

    // 12 行时受 CHAT_INPUT_MAX_ROWS (8) 上限约束，框高 = 10，并显示行号提示
    let twelve = (1..=12).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
    app.editor.set_text(twelve);
    terminal.draw(|f| app.render(f)).unwrap();
    assert_eq!(
        app.last_hits.input_area.height,
        crate::ui::CHAT_INPUT_MAX_ROWS + 2
    );
    let rendered = buffer_to_text(terminal.backend().buffer(), 120, 40);
    assert!(
        rendered.contains("第 12/12 行"),
        "missing overflow line counter:\n{rendered}"
    );
}

#[test]
fn resume_picker_shows_current_dir_scope_timestamps_and_switches_to_all() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    app.editor.set_text("/resume");
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let cur_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
    assert!(cur_view.contains("当前目录"), "missing current dir tab:\n{cur_view}");
    assert!(cur_view.contains("全部会话"), "missing all sessions tab:\n{cur_view}");
    assert!(cur_view.contains("[当前目录]"), "missing current dir badge:\n{cur_view}");

    // Tab 切换到全部会话，应能看到其他目录的会话
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    terminal.draw(|f| app.render(f)).unwrap();
    let all_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
    assert!(
        all_view.contains("Android SystemUI"),
        "switching to All scope should show other-dir session:\n{all_view}"
    );
}

#[test]
fn busy_bottom_status_renders_sai_companion_and_status_label() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    app.busy = true;
    app.transcript.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
    app.transcript.append_thought("checking animation state");

    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let view = buffer_to_text(terminal.backend().buffer(), 120, 36);
    assert!(
        view.contains('‿'),
        "busy bottom pulse line should render sai companion face:\n{view}"
    );
    assert!(
        view.contains("思考中"),
        "busy bottom pulse line should reflect active thinking status:\n{view}"
    );
    assert!(
        view.contains("3s"),
        "busy bottom pulse line should show calm integer seconds:\n{view}"
    );
    assert!(
        view.contains("Esc 中断"),
        "busy bottom pulse line should keep right-aligned Esc hint:\n{view}"
    );
    // 验证状态行紧贴输入框上方，且与正文底部留有 1 行空白分隔
    let pulse_y = app.last_hits.input_area.y.saturating_sub(1);
    assert_eq!(
        app.last_hits.body.bottom() + 1,
        pulse_y,
        "body bottom should keep 1 blank row above pulse line"
    );
    let lines: Vec<&str> = view.lines().collect();
    let pulse_row = lines[usize::from(pulse_y)];
    assert!(
        pulse_row.contains("思考中") && pulse_row.contains("Esc 中断"),
        "pulse line should sit directly above input box (row {pulse_y}): {pulse_row:?}"
    );
}

#[test]
fn hover_highlights_sidebar_items_without_affecting_body_fold_rows() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    // 1. 悬停侧边栏最近会话条目：触发重绘并给该行铺上 row_focus_bg
    let (sess_y, sess_id) = app.last_hits.sidebar.session_rows[1].clone();
    let side_x = app.last_hits.sidebar.area.x + 3;
    let moved = app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: side_x,
        row: sess_y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    assert!(moved);
    assert_eq!(app.hover, super::HoverTarget::SidebarSession(sess_id));

    // 同一条目内继续移动鼠标不触发无意义重绘
    let moved_same = app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: side_x + 2,
        row: sess_y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    assert!(!moved_same);

    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(side_x, sess_y)].bg, theme::palette().row_focus_bg);

    // 2. 悬停正文可折叠块（非行内链接）：不触发 hover，保持 HoverTarget::None
    if let Some((fold_y, _, _, _)) = app
        .last_hits
        .body_rows
        .iter()
        .find(|(_, _, clickable, links)| *clickable && links.is_empty())
        .cloned()
    {
        let _ = app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: app.last_hits.body.x + 1,
            row: fold_y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(app.hover, super::HoverTarget::None);
    }
}

#[test]
fn dragging_from_margins_never_copies_right_scrollbar_or_sidebar_divider() {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, r"D:\Code\litemiyu-acp".into(), None, true, true);
    app.auto_follow = true;
    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let bz = app.last_hits.body_text;
    let bar_col = app
        .last_hits
        .scrollbar_col
        .expect("demo body should have right scrollbar");
    let side_col = app
        .last_hits
        .sidebar_divider_col
        .expect("wide layout should have sidebar divider");
    assert!(bar_col >= bz.right(), "scrollbar must sit strictly right of body_text");
    assert!(side_col > bar_col, "sidebar divider must sit right of scrollbar");

    // 1) 从左侧留白边距 (col=0) 起手拖到右侧边栏内部及输入框下方：不能带入滚动条 ┃/│、侧栏 │、侧栏文字或输入框边框
    app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 0,
        row: bz.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: side_col + 10,
        row: bz.bottom() + 2,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    let sel = app.selection.expect("margin drag should create selection");
    assert_eq!(sel.zone, bz, "left-margin drag must clamp to body_text");
    let picked = app.last_hits.extract_selection(sel);
    assert!(!picked.is_empty());
    assert!(
        !picked.contains('┃') && !picked.contains('╭') && !picked.contains('❯'),
        "copied text must exclude right scrollbar and input border:\n{picked}"
    );
    // 表格以外的正文/代码行绝不能带入右侧滚动条 `│` 或侧栏分隔竖线 `│`
    for line in picked.lines() {
        if !line.starts_with('│') && !line.starts_with('┌') && !line.starts_with('├') && !line.starts_with('└') {
            assert!(
                !line.contains('│') && !line.contains('┃'),
                "non-table line must not include right decorative bar: {line:?}"
            );
        }
    }
    assert!(
        !picked.contains("工作区") && !picked.contains("最近会话"),
        "copied text must not bleed into sidebar:\n{picked}"
    );

    // 2) 从滚动条右侧留白边距反向往左拖选底部代码块与正文：同样绝不能带入 ┃ 或 │
    let right_margin_col = (bar_col + 1).min(side_col.saturating_sub(1));
    app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: right_margin_col,
        row: bz.bottom().saturating_sub(1),
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        column: bz.x,
        row: bz.bottom().saturating_sub(10),
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    let rev_sel = app.selection.expect("right-margin drag should create selection");
    assert_eq!(rev_sel.zone, bz, "right-margin drag must clamp to body_text");
    let rev_picked = app.last_hits.extract_selection(rev_sel);
    assert!(
        !rev_picked.is_empty() && !rev_picked.contains('┃') && !rev_picked.contains('│'),
        "reverse drag from right margin must exclude decorative bars:\n{rev_picked}"
    );
}

#[test]
fn background_task_badge_appears_after_turn_finishes_and_clears_on_completion() {
    use crate::acp::{AcpEvent, SessionUpdate};

    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, "/tmp/demo".into(), None, true, true);
    app.connecting = false;
    app.busy = true;

    // 1. 收到一个长时间运行工具调用，回合结束时仍处于 in_progress
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::ToolCall {
            call_id: "call_bg_1".into(),
            name: "run_command".into(),
            title: "sleep 10 && echo BG_OK".into(),
            kind: "execute".into(),
            status: "in_progress".into(),
            raw_input: Some(serde_json::json!({
                "CommandLine": "sleep 10 && echo BG_OK",
                "IsDaemon": false
            })),
            output: None,
            diffs: Vec::new(),
        },
    });
    app.handle_acp_event(AcpEvent::PromptFinished {
        prompt_id: 0,
        stop_reason: "end_turn".into(),
        usage: None,
    });
    assert!(!app.busy);
    assert!(app.has_background_tasks());

    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let screen_text = buffer_to_text(terminal.backend().buffer(), 140, 40);
    assert!(
        screen_text.contains("后台任务 1"),
        "idle pulse line should show running background task badge:\n{screen_text}"
    );
    assert!(
        screen_text.contains("后台 1"),
        "footer should show compact background count:\n{screen_text}"
    );
    assert_eq!(
        app.last_hits.sidebar.bg_rows.len(),
        1,
        "sidebar should expose clickable row for running background task"
    );
    assert_eq!(
        app.last_hits.screen_snapshot.height, 40,
        "flat screen snapshot should cover terminal height without per-cell String allocations"
    );

    // 2. 收到后台完成通知后标记自动清除
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::ToolCallUpdate {
            call_id: "call_bg_1".into(),
            name: None,
            title: None,
            kind: None,
            status: Some("completed".into()),
            raw_input: None,
            output: Some("BG_OK".into()),
            diffs: Vec::new(),
        },
    });
    assert!(!app.has_background_tasks());
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(app.last_hits.sidebar.bg_rows.is_empty());
}

#[test]
fn async_followup_keeps_pulse_animation_and_emits_turn_done_line() {
    use std::time::{Duration, Instant};

    use crate::acp::{AcpEvent, SessionUpdate};

    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, "/tmp/demo".into(), None, true, true);
    app.connecting = false;
    app.busy = false;

    // 1. 回合已空闲时收到服务端异步续答消息：自动进入生成动效活跃态并渲染底行流光
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::AgentMessageChunk("后台任务已完成，结果正常。".into()),
    });
    assert!(app.is_generating());

    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let active_view = buffer_to_text(terminal.backend().buffer(), 140, 40);
    assert!(
        active_view.contains('‿') && active_view.contains("正在生成"),
        "async follow-up should display bottom pulse animation:\n{active_view}"
    );

    // 2. 收到末尾 usage_update 并静默超时后自动收尾，落盘 ▣ 轮次结束行并退出活跃态
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::Usage {
            used: 20_000,
            size: 1_000_000,
        },
    });
    app.async_turn_last_event = Some(Instant::now() - Duration::from_secs(1));
    assert!(app.tick_timers());
    assert!(!app.is_generating());

    let last = app.transcript.entries.last().expect("has assistant entry");
    assert!(
        matches!(&last.kind, crate::transcript::EntryKind::Assistant { done_line: Some(m) } if m.starts_with('▣')),
        "async follow-up must emit ▣ turn completion line"
    );
}

#[test]
fn finish_turn_emits_done_line_even_when_turn_ends_with_tool_call() {
    use crate::acp::{AcpEvent, SessionUpdate};

    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, "/tmp/demo".into(), None, true, true);
    app.connecting = false;
    app.busy = true;
    app.transcript.push_user("检查构建".into(), Vec::new());
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::ToolCall {
            call_id: "c_only_tool".into(),
            name: "run_command".into(),
            title: "cargo check".into(),
            kind: "execute".into(),
            status: "completed".into(),
            raw_input: None,
            output: Some("ok".into()),
            diffs: Vec::new(),
        },
    });
    app.handle_acp_event(AcpEvent::PromptFinished {
        prompt_id: 0,
        stop_reason: "end_turn".into(),
        usage: None,
    });

    let last = app.transcript.entries.last().expect("has done line entry");
    assert!(
        matches!(&last.kind, crate::transcript::EntryKind::Assistant { done_line: Some(m) } if m.starts_with('▣')),
        "tool-only turn must still emit ▣ turn completion line"
    );
}

#[test]
fn esc_cancels_active_async_followup_and_emits_cancelled_line() {
    use crate::acp::{AcpEvent, SessionUpdate};

    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, "/tmp/demo".into(), None, true, true);
    app.connecting = false;
    app.busy = false;
    app.handle_acp_event(AcpEvent::Update {
        session_id: "demo-8a9f2c11".into(),
        update: SessionUpdate::AgentMessageChunk("异步续答中…".into()),
    });
    assert!(app.is_generating());

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(!app.is_generating());
    let last = app.transcript.entries.last().expect("has cancelled done line");
    assert!(
        matches!(&last.kind, crate::transcript::EntryKind::Assistant { done_line: Some(m) } if m.contains("已中断")),
        "pressing Esc during async follow-up should finish turn as cancelled"
    );
}

#[test]
fn completion_menu_supports_slash_commands_hash_skills_and_at_files() {
    let root = std::env::temp_dir().join(format!(
        "litemiyu_ui_completion_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agents/skills/demo-skill")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join(".agents/skills/demo-skill/SKILL.md"),
        "---\nname: demo-skill\ndescription: Skill demo for popup\n---\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, root.to_string_lossy().into_owned(), None, true, true);
    let backend = TestBackend::new(120, 36);
    let mut terminal = Terminal::new(backend).unwrap();

    // 1. `@` 触发工作区文件搜索，Enter 仅补全路径不提交
    app.editor.set_text("帮我看下 @main");
    terminal.draw(|f| app.render(f)).unwrap();
    let file_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
    assert!(
        file_view.contains("工作区文件") && file_view.contains("@main.rs"),
        "expected @ file completion popup:\n{file_view}"
    );
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(app.editor.text(), "帮我看下 @src/main.rs ");

    // 2. `#` 触发 Skills 搜索，Tab 插入，Esc 优先收起弹窗不清空输入
    app.editor.set_text("请使用 #demo-sk");
    terminal.draw(|f| app.render(f)).unwrap();
    let skill_view = buffer_to_text(terminal.backend().buffer(), 120, 36);
    assert!(
        skill_view.contains("Skills") && skill_view.contains("#demo-skill"),
        "expected # skill completion popup:\n{skill_view}"
    );
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(app.editor.text(), "请使用 #demo-sk");
    assert!(app.active_completion_menu().is_none());

    app.editor.insert_char('i');
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(app.editor.text(), "请使用 #demo-skill ");

    let _ = std::fs::remove_dir_all(&root);
}
