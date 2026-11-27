//! 折叠行与子代理摘要的终端交互回归测试。

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

use crate::acp::{AcpEvent, SessionUpdate, ToolDiff};
use crate::app::App;
use crate::transcript::FoldSection;
use crate::ui::HoverTarget;

/// 创建空白会话与指定 `width`、`height` 的测试终端。
fn screen(width: u16, height: u16) -> (App, Terminal<TestBackend>) {
    let mut app = App::new(None, "workspace".into(), None, true, false);
    app.connecting = false;
    app.auto_follow = false;
    app.transcript.session_id = Some("parent".into());
    (app, Terminal::new(TestBackend::new(width, height)).unwrap())
}

/// 在 `row` 和 `col` 处发送 `kind` 鼠标事件，返回是否需要刷新。
fn mouse(app: &mut App, kind: MouseEventKind, col: u16, row: u16) -> bool {
    app.handle_mouse(MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// 查找当前屏幕中指定条目和 `section` 的折叠提示行。
fn marker(app: &App, entry: u64, section: FoldSection) -> Option<u16> {
    app.last_hits
        .fold_rows
        .iter()
        .find(|(_, target)| **target == (entry, section))
        .map(|(row, _)| *row)
}

/// 构造 `count` 行互不折行的短文本。
fn numbered(count: usize) -> String {
    (0..count)
        .map(|number| format!("line {number:03}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 验证真实终端行内按钮热区、纯前景悬停高亮（无背景色）、左键步进展开与右键全部展开。
#[test]
fn fold_marker_hover_left_right_and_reset() {
    use crate::ui::rows::InlineAction;

    let (mut app, mut terminal) = screen(84, 50);
    app.transcript.append_thought(&numbered(48));
    let entry = app.transcript.entries[0].id;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = marker(&app, entry, FoldSection::Thought).unwrap();
    let base_x = app.last_hits.body.x;
    let initial = app.last_hits.total_body_rows;
    let bar_text = app
        .last_hits
        .screen_snapshot
        .slice_row_text(row, base_x, 80);
    assert!(
        bar_text.contains("前面还有 45 行")
            && bar_text.contains("+20 行")
            && bar_text.contains("展开全部"),
        "unexpected thought action bar: {bar_text}"
    );

    // 1. 悬停「+20 行」按钮：仅高亮前景文字，不铺设任何背景色
    let step_btn = app.last_hits.inline_buttons[&row]
        .1
        .iter()
        .find(|b| matches!(b.action, InlineAction::RevealStep { .. }))
        .copied()
        .expect("step button");
    let step_col = base_x + step_btn.col_start;
    let before_bg = terminal.backend().buffer()[(step_col, row)].bg;
    assert!(mouse(&mut app, MouseEventKind::Moved, step_col, row));
    assert!(matches!(
        app.hover,
        HoverTarget::InlineButton {
            action: InlineAction::RevealStep {
                section: FoldSection::Thought,
                step: 20
            },
            ..
        }
    ));
    terminal.draw(|frame| app.render(frame)).unwrap();
    let cell = &terminal.backend().buffer()[(step_col, row)];
    assert_eq!(cell.bg, before_bg, "inline fold hover must not paint background");
    assert_ne!(cell.bg, crate::theme::palette().row_focus_bg);
    assert_eq!(
        cell.fg,
        crate::theme::accent_bold()
            .fg
            .unwrap_or(crate::theme::palette().accent)
    );

    // 2. 点击「+20 行」两次：依次剩余 25 行、5 行；剩余 5 行时不再出现 `+20 行`
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), step_col, row);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), step_col, row);
    terminal.draw(|frame| app.render(frame)).unwrap();
    // 展开后底部会追加 1 行「▴ 收起」
    assert_eq!(app.last_hits.total_body_rows, initial + 20 + 1);

    let row2 = marker(&app, entry, FoldSection::Thought).unwrap();
    let step_btn2 = app.last_hits.inline_buttons[&row2]
        .1
        .iter()
        .find(|b| matches!(b.action, InlineAction::RevealStep { .. }))
        .copied()
        .expect("second step button");
    let step_col2 = base_x + step_btn2.col_start;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), step_col2, row2);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), step_col2, row2);
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row3 = marker(&app, entry, FoldSection::Thought).unwrap();
    let bar_last = app
        .last_hits
        .screen_snapshot
        .slice_row_text(row3, base_x, 80);
    assert!(
        bar_last.contains("前面还有 5 行")
            && bar_last.contains("展开前 5 行")
            && !bar_last.contains("+20 行"),
        "small remainder must not show +20 button: {bar_last}"
    );

    // 3. 右键整行依然兼容一键展开全部，收起后重置为初始行数
    assert!(!mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        base_x.saturating_sub(1),
        row3
    ));
    assert!(mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        base_x,
        row3
    ));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(marker(&app, entry, FoldSection::Thought).is_none());
    // 1 标题 + 48 正文 + 1 底部收起
    assert_eq!(app.last_hits.total_body_rows, 50);
    app.transcript.toggle_all_details(false);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.last_hits.total_body_rows, initial);
}

/// 验证命令、输出、文件读取（24 行起）和不同文件的 Diff 各自保留独立展开上限与步长。
#[test]
fn tool_folds_are_independent_and_count_visual_lines() {
    use crate::ui::rows::{MdCache, RowAction, build_body};
    let (mut app, _) = screen(90, 40);
    let text = numbered(660);
    let cmd = app.transcript.upsert_tool(
        "command".into(),
        Some("shell".into()),
        None,
        Some("execute".into()),
        None,
        Some(json!({"command": numbered(30)})),
        Some(numbered(52)),
        vec![],
    );
    let read = app.transcript.upsert_tool(
        "read".into(),
        Some("read_file".into()),
        None,
        Some("read".into()),
        None,
        Some(json!({"path":"test.txt"})),
        Some(numbered(180)),
        vec![],
    );
    let diffs = ["first.txt", "second.txt"].map(|path| ToolDiff {
        path: path.into(),
        old_text: String::new(),
        new_text: text.clone(),
    });
    let edit = app.transcript.upsert_tool(
        "edit".into(),
        Some("edit".into()),
        None,
        None,
        None,
        None,
        None,
        diffs.into(),
    );
    app.transcript.toggle_all_details(true);
    let mut cache = MdCache::new();
    let mut markers = |app: &App| {
        build_body(&app.transcript, &[], 90, false, 0, &mut cache)
            .rows
            .into_iter()
            .filter_map(|row| {
                if let RowAction::Fold(section) = row.action {
                    let full: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
                    Some(((row.entry_id, section), full))
                } else {
                    None
                }
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let before = markers(&app);
    assert_eq!(before.len(), 5);
    for (target, count) in [
        ((cmd, FoldSection::ToolCommand), 22),
        ((cmd, FoldSection::ToolOutput), 28),
        ((read, FoldSection::ToolReadPreview), 156),
        ((edit, FoldSection::ToolDiff(0)), 60),
        ((edit, FoldSection::ToolDiff(1)), 60),
    ] {
        assert!(
            before[&target].contains(&format!(" {count} 行")),
            "expected {count} in {:?}",
            before[&target]
        );
    }
    app.transcript
        .reveal_fold(edit, FoldSection::ToolDiff(0), false);
    app.transcript
        .reveal_fold(cmd, FoldSection::ToolCommand, true);
    let after = markers(&app);
    assert!(
        after[&(edit, FoldSection::ToolDiff(0))].contains(" 20 行")
            && after[&(edit, FoldSection::ToolDiff(0))].contains("展开剩余 20 行")
            && !after[&(edit, FoldSection::ToolDiff(0))].contains("+40 行"),
        "diff remainder <= 40 should not show +40: {:?}",
        after[&(edit, FoldSection::ToolDiff(0))]
    );
    assert_eq!(
        after[&(edit, FoldSection::ToolDiff(1))],
        before[&(edit, FoldSection::ToolDiff(1))]
    );
    assert!(!after.contains_key(&(cmd, FoldSection::ToolCommand)));
    assert_eq!(
        after[&(cmd, FoldSection::ToolOutput)],
        before[&(cmd, FoldSection::ToolOutput)]
    );
}

/// 验证增量 ACP 更新合并、宽屏侧栏点击定位、窄屏卡片与断连清理。
#[test]
fn subagent_summary_survives_sparse_updates_and_narrow_layout() {
    let (mut app, mut terminal) = screen(140, 42);
    let send = |app: &mut App, update| {
        app.handle_acp_event(AcpEvent::Update {
            session_id: "parent".into(),
            update,
        })
    };
    send(
        &mut app,
        SessionUpdate::ToolCall {
            call_id: "launch".into(),
            name: "invoke_subagent".into(),
            title: "调用".into(),
            kind: "other".into(),
            status: "in_progress".into(),
            raw_input: Some(json!({"Role":"资料检索","Model":"fixture","Prompt":"核对版本"})),
            output: None,
            diffs: vec![],
        },
    );
    send(
        &mut app,
        SessionUpdate::ToolCallUpdate {
            call_id: "launch".into(),
            name: None,
            title: None,
            kind: None,
            status: Some("completed".into()),
            raw_input: None,
            output: Some(r#"{"conversationId":"child"}"#.into()),
            diffs: vec![],
        },
    );
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.last_hits.sidebar.bg_rows.is_empty());
    let (row, entry) = app.last_hits.sidebar.subagent_rows[0];
    let col = app.last_hits.sidebar.area.x + 2;
    assert!(
        app.last_hits
            .screen_snapshot
            .slice_row_text(row, col, 140)
            .contains("已启动 · 资料检索")
    );
    mouse(&mut app, MouseEventKind::Moved, col, row);
    assert_eq!(app.hover, HoverTarget::SidebarSubagent(entry));
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), col, row);
    let mut narrow = Terminal::new(TestBackend::new(70, 30)).unwrap();
    narrow.draw(|frame| app.render(frame)).unwrap();
    let text: String = (0..30)
        .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, 70))
        .collect();
    assert!(
        text.contains("子代理 · 已启动 · 资料检索")
            && text.contains("模型：fixture")
            && text.contains("核对版本")
    );
    assert!(!text.contains("conversationId"));
    app.handle_acp_event(AcpEvent::Disconnected("测试".into()));
    assert!(app.subagents.agents.is_empty());
}

/// 验证 agy `update_steps` 转为结构化任务清单（时间线原地更新 + 宽屏侧栏 + 窄屏停靠条与 Ctrl+T 展开）。
#[test]
fn agy_update_steps_renders_in_timeline_sidebar_and_narrow_todo_bar() {
    use crossterm::event::{KeyCode, KeyEvent};

    let (mut app, mut wide_term) = screen(140, 42);
    app.transcript.push_user("请完成重构与测试".into(), Vec::new());

    // 1. 首次收到 agy `update_steps`：不应产生空白工具卡片或后台任务
    app.handle_acp_event(AcpEvent::Update {
        session_id: "parent".into(),
        update: SessionUpdate::ToolCall {
            call_id: "call_steps_1".into(),
            name: "update_steps".into(),
            title: "Run update_steps".into(),
            kind: "other".into(),
            status: "completed".into(),
            raw_input: Some(json!({
                "steps": [
                    { "title": "排查协议报文", "status": "done" },
                    { "title": "实现任务清单组件", "status": "in_progress" },
                    { "title": "补充单元测试", "status": "pending" }
                ]
            })),
            output: None,
            diffs: vec![],
        },
    });
    assert!(app.background.running_tasks().is_empty());
    assert_eq!(app.transcript.latest_plan.len(), 3);
    assert!(
        !app.transcript
            .entries
            .iter()
            .any(|e| matches!(e.kind, crate::transcript::EntryKind::Tool { .. })),
        "update_steps should not emit a generic tool entry"
    );

    // 2. 同回合二次流式更新（ToolCall 无入参 + ToolCallUpdate 补齐 steps）：应原地更新而非堆叠两张清单
    app.handle_acp_event(AcpEvent::Update {
        session_id: "parent".into(),
        update: SessionUpdate::ToolCall {
            call_id: "call_steps_2".into(),
            name: "update_steps".into(),
            title: "Run update_steps".into(),
            kind: "other".into(),
            status: "in_progress".into(),
            raw_input: None,
            output: None,
            diffs: vec![],
        },
    });
    app.handle_acp_event(AcpEvent::Update {
        session_id: "parent".into(),
        update: SessionUpdate::ToolCallUpdate {
            call_id: "call_steps_2".into(),
            name: None,
            title: None,
            kind: None,
            status: Some("completed".into()),
            raw_input: Some(json!({
                "steps": [
                    { "title": "排查协议报文", "status": "done" },
                    { "title": "实现任务清单组件", "status": "done" },
                    { "title": "补充单元测试", "status": "in_progress" }
                ]
            })),
            output: None,
            diffs: vec![],
        },
    });
    let plan_entries: Vec<&crate::transcript::Entry> = app
        .transcript
        .entries
        .iter()
        .filter(|e| matches!(e.kind, crate::transcript::EntryKind::Plan { .. }))
        .collect();
    assert_eq!(plan_entries.len(), 1, "repeated update_steps in same turn must update in-place");
    // 正文滚动流里的任务记录默认保持单行折叠态，完整任务清单固定展示在底部
    assert!(
        matches!(plan_entries[0].kind, crate::transcript::EntryKind::Plan { expanded: false, .. }),
        "timeline plan entry should default to single-line collapsed summary"
    );

    // 3. 宽屏下底部始终固定展示展开态任务清单，同时右侧边栏同步显示「任务清单 (2/3)」
    wide_term.draw(|f| app.render(f)).unwrap();
    let wide_text: String = (0..42)
        .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, 140))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(wide_text.contains("任务清单 (2/3)"), "missing sidebar todo section:\n{wide_text}");
    assert!(
        wide_text.contains("任务 2/3") && wide_text.contains("Ctrl+T 收起"),
        "wide screen should dock expanded todo bar at bottom:\n{wide_text}"
    );
    assert!(wide_text.contains("补充单元测试"), "missing active step in view:\n{wide_text}");
    let todo_rect = app.last_hits.sidebar.todo_area;
    assert!(todo_rect.width > 0 && todo_rect.height >= 4);
    assert!(mouse(&mut app, MouseEventKind::Moved, todo_rect.x + 1, todo_rect.y));
    assert_eq!(app.hover, HoverTarget::SidebarTodo);

    // 4. 窄屏（无侧栏）同样固定在底部展示展开清单，按 Ctrl+T 或点击标题行可收起为单行并再展开
    let mut narrow_term = Terminal::new(TestBackend::new(84, 28)).unwrap();
    narrow_term.draw(|f| app.render(f)).unwrap();
    let expanded_narrow: String = (0..28)
        .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, 84))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        expanded_narrow.contains("任务 2/3")
            && expanded_narrow.contains("Ctrl+T 收起")
            && expanded_narrow.contains("排查协议报文"),
        "narrow screen should render expanded docked todo bar by default:\n{expanded_narrow}"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(!app.todo_bar_expanded);
    narrow_term.draw(|f| app.render(f)).unwrap();
    let collapsed_narrow: String = (0..28)
        .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, 84))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        collapsed_narrow.contains("任务 2/3") && collapsed_narrow.contains("Ctrl+T 展开"),
        "collapsed docked todo bar should stay as single line:\n{collapsed_narrow}"
    );
    let toggle_rect = app.last_hits.todo_bar_toggle;
    assert!(toggle_rect.width > 0 && toggle_rect.height == 1);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), toggle_rect.x + 1, toggle_rect.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), toggle_rect.x + 1, toggle_rect.y);
    assert!(app.todo_bar_expanded, "clicking docked todo bar should expand it again");

    // 5. 矮终端（16 行高）展开 10 条长列表时，聚焦步骤与上下省略提示必须完整呈现且不丢项
    let long_steps: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            json!({
                "title": format!("长步骤 {i}"),
                "status": if i < 5 { "done" } else if i == 5 { "in_progress" } else { "pending" }
            })
        })
        .collect();
    app.handle_acp_event(AcpEvent::Update {
        session_id: "parent".into(),
        update: SessionUpdate::ToolCall {
            call_id: "call_steps_long".into(),
            name: "update_steps".into(),
            title: "Run update_steps".into(),
            kind: "other".into(),
            status: "completed".into(),
            raw_input: Some(json!({ "steps": long_steps })),
            output: None,
            diffs: vec![],
        },
    });
    let mut short_term = Terminal::new(TestBackend::new(84, 16)).unwrap();
    short_term.draw(|f| app.render(f)).unwrap();
    let short_text: String = (0..16)
        .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, 84))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        short_text.contains("长步骤 5")
            && short_text.contains("上方还有")
            && short_text.contains("其余"),
        "short terminal expanded todo bar must keep focused step and overflow counts:\n{short_text}"
    );

    // 6. 空数组清空任务清单时，正文时间线中的旧任务卡片同步清理
    app.handle_acp_event(AcpEvent::Update {
        session_id: "parent".into(),
        update: SessionUpdate::ToolCall {
            call_id: "call_steps_clear".into(),
            name: "update_steps".into(),
            title: "Run update_steps".into(),
            kind: "other".into(),
            status: "completed".into(),
            raw_input: Some(json!({ "steps": [] })),
            output: None,
            diffs: vec![],
        },
    });
    assert!(app.transcript.latest_plan.is_empty());
    assert!(
        !app.transcript
            .entries
            .iter()
            .any(|e| matches!(e.kind, crate::transcript::EntryKind::Plan { .. })),
        "clearing plan with empty steps should remove stale timeline plan card"
    );
}

/// 验证模态选择器（状态/恢复会话）、权限审批抽屉、代理提问抽屉与补全菜单全面支持鼠标悬停、单击与外部关闭。
#[test]
fn overlay_popups_and_drawers_support_mouse_hover_click_and_dismiss() {
    use crate::acp::{
        ElicitationField, ElicitationFieldKind, ElicitationOption, ElicitationRequest,
        PermissionOption, PermissionRequest, SessionSummary,
    };
    use crate::ui::overlay::{PickerKind, ResumeScope};

    let (mut app, mut term) = screen(120, 36);
    app.transcript.session_id = Some("121fe1e7-7919-4e98-983d-03aa2520051e".into());
    app.transcript.context_used = 102_200;
    app.transcript.context_window = 1_000_000;
    app.transcript.push_user("测试弹窗鼠标交互".into(), Vec::new());

    // 1. 状态弹窗（与用户截图同款）：点击底栏用量打开、悬停切高亮、点击外部关闭、点击选项直接执行
    term.draw(|f| app.render(f)).unwrap();
    let usage_rect = app.last_hits.usage_cell;
    assert!(usage_rect.width > 0, "usage cell should be clickable");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), usage_rect.x + 1, usage_rect.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), usage_rect.x + 1, usage_rect.y);
    assert!(app.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Status));
    term.draw(|f| app.render(f)).unwrap();
    let status_items = app.last_hits.overlay.picker.items.clone();
    assert_eq!(status_items.len(), 3, "status picker should expose 3 clickable rows");
    let third_row = status_items[2].0;
    assert!(mouse(&mut app, MouseEventKind::Moved, third_row.x + 2, third_row.y));
    assert_eq!(app.picker.as_ref().unwrap().selected, 2);
    // 点击弹窗外部空白处应直接关闭弹窗
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 0, 0);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 0, 0);
    assert!(app.picker.is_none(), "clicking outside status picker should close it");

    // 重新通过底栏打开状态弹窗并单击「复制完整会话 ID」执行动作
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), usage_rect.x + 1, usage_rect.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), usage_rect.x + 1, usage_rect.y);
    term.draw(|f| app.render(f)).unwrap();
    let copy_row = app.last_hits.overlay.picker.items[2].0;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), copy_row.x + 2, copy_row.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), copy_row.x + 2, copy_row.y);
    assert!(app.picker.is_none());
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|(m, _, _)| m.contains("已复制会话 ID")),
        "clicking copy_id row should execute choice"
    );

    // 2. `/resume` 弹窗：单击顶部范围标签页切换当前目录/全部会话
    app.recent_sessions = vec![
        SessionSummary {
            session_id: "sess-local".into(),
            cwd: app.cwd.clone(),
            title: "当前目录会话".into(),
            updated_at: chrono::Local::now().to_rfc3339(),
        },
        SessionSummary {
            session_id: "sess-other".into(),
            cwd: "/tmp/other-workspace".into(),
            title: "其他目录会话".into(),
            updated_at: chrono::Local::now().to_rfc3339(),
        },
    ];
    app.submit_text("/resume".into());
    term.draw(|f| app.render(f)).unwrap();
    assert_eq!(app.picker.as_ref().unwrap().scope, ResumeScope::CurrentDir);
    let all_tab = app
        .last_hits
        .overlay
        .picker
        .resume_tabs
        .iter()
        .find(|(_, s)| *s == ResumeScope::All)
        .map(|(r, _)| *r)
        .expect("resume All tab hitbox");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), all_tab.x + 1, all_tab.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), all_tab.x + 1, all_tab.y);
    assert_eq!(app.picker.as_ref().unwrap().scope, ResumeScope::All);
    app.picker = None;

    // 3. 权限审批抽屉：鼠标悬停切高亮、单击直接确认并关闭抽屉
    app.permission = Some((
        PermissionRequest {
            rpc_id: json!(9),
            session_id: "121fe1e7-7919-4e98-983d-03aa2520051e".into(),
            tool_call_id: "call_perm_1".into(),
            title: "Run cargo test".into(),
            kind: "execute".into(),
            raw_input: Some(json!({ "CommandLine": "cargo test" })),
            options: vec![
                PermissionOption {
                    option_id: "allow_once".into(),
                    name: "允许本次".into(),
                    kind: "allow_once".into(),
                },
                PermissionOption {
                    option_id: "reject".into(),
                    name: "拒绝".into(),
                    kind: "reject".into(),
                },
            ],
        },
        0,
    ));
    term.draw(|f| app.render(f)).unwrap();
    let perm_opts = app.last_hits.overlay.permission.options.clone();
    assert_eq!(perm_opts.len(), 2);
    assert!(mouse(&mut app, MouseEventKind::Moved, perm_opts[1].0.x + 2, perm_opts[1].0.y));
    assert_eq!(app.permission.as_ref().unwrap().1, 1);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), perm_opts[0].0.x + 2, perm_opts[0].0.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), perm_opts[0].0.x + 2, perm_opts[0].0.y);
    assert!(app.permission.is_none(), "clicking permission option should answer and close drawer");

    // 4. 代理提问表单抽屉：单击页签切题 + 单击多选勾选
    app.handle_acp_event(AcpEvent::ElicitationRequested(ElicitationRequest {
        rpc_id: json!(11),
        session_id: "121fe1e7-7919-4e98-983d-03aa2520051e".into(),
        elicitation_id: String::new(),
        mode: "form".into(),
        message: "2 questions".into(),
        url: None,
        fields: vec![
            ElicitationField {
                key: "q0".into(),
                title: "First".into(),
                description: String::new(),
                required: true,
                kind: ElicitationFieldKind::SingleSelect(vec![ElicitationOption {
                    value: "a".into(),
                    label: "Option A".into(),
                    description: String::new(),
                }]),
                other_key: None,
                other_title: String::new(),
            },
            ElicitationField {
                key: "q1".into(),
                title: "Second".into(),
                description: String::new(),
                required: true,
                kind: ElicitationFieldKind::MultiSelect {
                    options: vec![
                        ElicitationOption {
                            value: "x".into(),
                            label: "Check X".into(),
                            description: String::new(),
                        },
                        ElicitationOption {
                            value: "y".into(),
                            label: "Check Y".into(),
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
    term.draw(|f| app.render(f)).unwrap();
    let tab2 = app.last_hits.overlay.elicitation.tabs[1].0;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), tab2.x + 1, tab2.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), tab2.x + 1, tab2.y);
    assert_eq!(app.elicitation.as_ref().unwrap().active_field, 1);
    term.draw(|f| app.render(f)).unwrap();
    let opt_y = app.last_hits.overlay.elicitation.options[1].0;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), opt_y.x + 2, opt_y.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), opt_y.x + 2, opt_y.y);
    assert_eq!(app.elicitation.as_ref().unwrap().answers[1].checked, vec![false, true]);
    app.elicitation = None;

    // 5. 输入框 `/` 补全浮层：鼠标悬停切高亮、单击直接执行命令
    for ch in "/the".chars() {
        app.editor.insert_char(ch);
    }
    term.draw(|f| app.render(f)).unwrap();
    let comp_items = app.last_hits.overlay.completion.items.clone();
    assert!(!comp_items.is_empty(), "completion menu should have clickable items");
    let theme_row = comp_items[0].0;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), theme_row.x + 2, theme_row.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), theme_row.x + 2, theme_row.y);
    assert!(
        app.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Theme),
        "clicking /theme in completion menu should open theme picker"
    );
}

/// 验证优化后的启动首页在 84 列与 62 列下均完整展示快捷键（绝不截断「/help 帮助」），且支持点击打开浮层与最近会话直达。
#[test]
fn home_screen_never_truncates_shortcuts_and_supports_clicking_actions_and_sessions() {
    use crate::acp::SessionSummary;

    crate::theme::init(crate::theme::ThemePreset::MiyuLavender, true);
    let mut app = App::new(
        None,
        "/home/snemc/workspace/agy-tui".into(),
        None,
        true,
        false,
    );
    app.connecting = false;
    app.agent_banner = "Antigravity ACP Server v1.5.17".into();
    app.fallback_model_name = "Gemini 4 Neon".into();
    app.recent_sessions = vec![SessionSummary {
        session_id: "sess-recent-1".into(),
        cwd: "/home/snemc/workspace/agy-tui".into(),
        title: "排查 agy todo 展示与启动页布局".into(),
        updated_at: chrono::Local::now().to_rfc3339(),
    }];

    for (w, h) in [(84u16, 26u16), (62u16, 24u16)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let text: String = (0..h)
            .map(|row| app.last_hits.screen_snapshot.slice_row_text(row, 0, w))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("agy-tui"), "missing title at {w}x{h}:\n{text}");
        assert!(text.contains("/resume 恢复会话"), "missing resume hint at {w}x{h}:\n{text}");
        assert!(text.contains("/help 帮助"), "help hint must never truncate at {w}x{h}:\n{text}");
        assert!(
            text.contains("排查 agy todo 展示与启动页布局"),
            "missing recent session preview at {w}x{h}:\n{text}"
        );
    }

    // 点击首页 /help 快捷项应直接弹出帮助浮层
    let mut term = Terminal::new(TestBackend::new(84, 26)).unwrap();
    term.draw(|f| app.render(f)).unwrap();
    let help_rect = app
        .last_hits
        .home
        .actions
        .iter()
        .find(|(_, a)| *a == crate::ui::HomeAction::Help)
        .map(|(r, _)| *r)
        .expect("home help action hitbox");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), help_rect.x + 1, help_rect.y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), help_rect.x + 1, help_rect.y);
    assert!(
        app.picker
            .as_ref()
            .is_some_and(|p| p.kind == crate::ui::overlay::PickerKind::Help),
        "clicking /help on home screen should open help picker"
    );
}
