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

/// 验证真实终端热区、悬停提示、左键追加十行与右键展开剩余内容。
#[test]
fn fold_marker_hover_left_right_and_reset() {
    let (mut app, mut terminal) = screen(84, 50);
    app.transcript.append_thought(&numbered(27));
    let entry = app.transcript.entries[0].id;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let row = marker(&app, entry, FoldSection::Thought).unwrap();
    let col = app.last_hits.body_text.x;
    let initial = app.last_hits.total_body_rows;
    let snapshot = &app.last_hits.screen_snapshot;
    assert!(
        snapshot
            .slice_row_text(row, col, 80)
            .contains("已折叠前 24 行")
    );
    assert!(mouse(&mut app, MouseEventKind::Moved, col, row));
    assert_eq!(app.hover, HoverTarget::Fold(entry, FoldSection::Thought));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        app.last_hits
            .screen_snapshot
            .slice_row_text(row, col, 80)
            .contains("点击展开10行，右击全部展开")
    );
    assert_eq!(
        terminal.backend().buffer()[(col, row)].bg,
        crate::theme::palette().row_focus_bg
    );
    for revealed in [10, 20] {
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), col, row);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), col, row);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(app.last_hits.total_body_rows, initial + revealed);
    }
    assert!(!mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        col - 1,
        row
    ));
    assert!(mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        col,
        row
    ));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(marker(&app, entry, FoldSection::Thought).is_none());
    assert_eq!(app.last_hits.total_body_rows, 28);
    app.transcript.toggle_all_details(false);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.last_hits.total_body_rows, initial);
}

/// 验证命令、输出、文件读取和不同文件的 Diff 各自保留展开上限。
#[test]
fn tool_folds_are_independent_and_count_visual_lines() {
    use crate::ui::rows::{MdCache, RowAction, build_body};
    let (mut app, _) = screen(90, 40);
    let text = numbered(630);
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
        build_body(&app.transcript, &[], 90, 0, &mut cache)
            .rows
            .into_iter()
            .filter_map(|row| {
                if let RowAction::Fold(section) = row.action {
                    Some(((row.entry_id, section), row.spans[0].content.to_string()))
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
        ((read, FoldSection::ToolReadPreview), 20),
        ((edit, FoldSection::ToolDiff(0)), 30),
        ((edit, FoldSection::ToolDiff(1)), 30),
    ] {
        assert!(before[&target].contains(&format!(" {count} 行")));
    }
    app.transcript
        .reveal_fold(edit, FoldSection::ToolDiff(0), false);
    app.transcript
        .reveal_fold(cmd, FoldSection::ToolCommand, true);
    let after = markers(&app);
    assert!(after[&(edit, FoldSection::ToolDiff(0))].contains(" 20 行"));
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
