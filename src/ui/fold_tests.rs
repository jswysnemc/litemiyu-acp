//! 读取工具 24 行默认预览、查看完整文件、智能剩余按钮与正文防误收起的专项测试。

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

use crate::app::App;
use crate::theme::{self, ThemePreset};
use crate::transcript::FoldSection;
use crate::ui::HoverTarget;
use crate::ui::rows::InlineAction;

fn make_app(width: u16, height: u16) -> (App, Terminal<TestBackend>) {
    theme::init(ThemePreset::MiyuLavender, true);
    let mut app = App::new(None, "workspace".into(), None, true, false);
    app.connecting = false;
    app.auto_follow = false;
    app.transcript.session_id = Some("fold-test".into());
    (app, Terminal::new(TestBackend::new(width, height)).unwrap())
}

fn send_mouse(app: &mut App, kind: MouseEventKind, col: u16, row: u16) -> bool {
    app.handle_mouse(MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn click(app: &mut App, col: u16, row: u16) {
    send_mouse(app, MouseEventKind::Down(MouseButton::Left), col, row);
    send_mouse(app, MouseEventKind::Up(MouseButton::Left), col, row);
}

fn find_button(app: &App, target_entry: u64, pred: impl Fn(InlineAction) -> bool) -> Option<(u16, u16)> {
    let base_x = app.last_hits.body.x;
    for (&row, (entry_id, btns)) in &app.last_hits.inline_buttons {
        if *entry_id != target_entry {
            continue;
        }
        if let Some(btn) = btns.iter().find(|b| pred(b.action)) {
            return Some((base_x + btn.col_start, row));
        }
    }
    None
}

/// 验证局部读取默认展示 24 行、精准剩余文案、点击正文可收起、切换查看完整文件以及无背景前景高亮。
#[test]
fn read_tool_defaults_to_24_lines_supports_full_file_and_body_click_collapses() {
    let tmp_path = std::env::temp_dir().join(format!(
        "agy_tui_read_test_{}.rs",
        std::process::id()
    ));
    let content: String = (1..=220)
        .map(|i| format!("let line_{i:03} = {i};"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&tmp_path, &content).unwrap();

    let (mut app, mut term) = make_app(110, 48);
    let entry_id = app.transcript.upsert_tool(
        "read_partial".into(),
        Some("read_file".into()),
        Some(format!("Read {}", tmp_path.display())),
        Some("read".into()),
        Some("completed".into()),
        Some(json!({
            "Path": tmp_path.to_string_lossy(),
            "StartLine": 50,
            "EndLine": 95
        })),
        None,
        Vec::new(),
    );

    // 1. 首次点击顶部标题展开：默认只展示 24 行（L50..L73），剩余 22 行 <= 40，不显示 `+40 行`
    app.transcript.set_entry_expanded(entry_id, true);
    term.draw(|f| app.render(f)).unwrap();
    let screen1: String = (0..48)
        .map(|r| app.last_hits.screen_snapshot.slice_row_text(r, 0, 110))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen1.contains("let line_050 = 50;"), "missing start line:\n{screen1}");
    assert!(screen1.contains("let line_073 = 73;"), "missing 24th preview line:\n{screen1}");
    assert!(!screen1.contains("let line_074 = 74;"), "should cap initial read preview at 24 lines");
    assert!(
        screen1.contains("剩余 22 行")
            && screen1.contains("展开剩余 22 行")
            && !screen1.contains("+40 行")
            && screen1.contains("查看完整文件")
            && screen1.contains("▴ 收起"),
        "read bar should adapt to 22 remaining lines and offer full file + collapse:\n{screen1}"
    );

    // 2. 点击展开态的正文源码行可直接收起，随后重新展开继续后续操作
    let code_row = app
        .last_hits
        .body_rows
        .iter()
        .skip(2)
        .find(|(_, eid, clickable, links)| *eid == entry_id && *clickable && links.is_empty())
        .map(|(y, _, _, _)| *y)
        .expect("clickable code body row");
    let code_col = app.last_hits.body.x + 6;
    click(&mut app, code_col, code_row);
    assert!(
        matches!(
            app.transcript.entries[0].kind,
            crate::transcript::EntryKind::Tool { expanded: false, .. }
        ),
        "clicking code body row should collapse expanded entry"
    );
    app.transcript.set_entry_expanded(entry_id, true);
    term.draw(|f| app.render(f)).unwrap();

    // 3. 点击「展开剩余 22 行」后看到 L95，底部仍提供「查看完整文件」与「▴ 收起」
    let (all_x, all_y) = find_button(&app, entry_id, |a| {
        matches!(a, InlineAction::RevealAll { section: FoldSection::ToolReadPreview })
    })
    .expect("exact remainder expand button");
    click(&mut app, all_x, all_y);
    app.scroll = 10;
    term.draw(|f| app.render(f)).unwrap();
    let screen2: String = (0..48)
        .map(|r| app.last_hits.screen_snapshot.slice_row_text(r, 0, 110))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen2.contains("let line_095 = 95;"), "should reveal up to end of partial read:\n{screen2}");
    assert!(screen2.contains("查看完整文件") && screen2.contains("▴ 收起"));

    // 4. 点击「查看完整文件」切换为整份 220 行文件（自动重置分区折叠上限，不瞬时刷屏，且不再显示「查看完整文件」）
    let (full_x, full_y) = find_button(&app, entry_id, |a| a == InlineAction::ReadFullFile)
        .expect("view full file button");
    click(&mut app, full_x, full_y);
    assert!(app.transcript.is_read_full_file(entry_id));
    app.scroll = 0;
    term.draw(|f| app.render(f)).unwrap();
    let screen3: String = (0..48)
        .map(|r| app.last_hits.screen_snapshot.slice_row_text(r, 0, 110))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen3.contains("let line_001 = 1;"), "full file view should start from line 1:\n{screen3}");
    assert!(
        screen3.contains("剩余 196 行")
            && screen3.contains("+40 行")
            && screen3.contains("展开全部")
            && !screen3.contains("查看完整文件"),
        "full file view should paginate all 220 lines without hard cap:\n{screen3}"
    );

    // 5. 悬停「+40 行」仅高亮前景、绝不铺背景色
    let (step_x, step_y) = find_button(&app, entry_id, |a| {
        matches!(a, InlineAction::RevealStep { section: FoldSection::ToolReadPreview, step: 40 })
    })
    .expect("+40 step button");
    let prev_bg = term.backend().buffer()[(step_x, step_y)].bg;
    assert!(send_mouse(&mut app, MouseEventKind::Moved, step_x, step_y));
    assert!(matches!(app.hover, HoverTarget::InlineButton { .. }));
    term.draw(|f| app.render(f)).unwrap();
    assert_eq!(term.backend().buffer()[(step_x, step_y)].bg, prev_bg);
    assert_ne!(
        term.backend().buffer()[(step_x, step_y)].bg,
        theme::palette().row_focus_bg
    );

    // 6. 点击底部「▴ 收起」收起条目并重置完整文件模式
    let (col_x, col_y) = find_button(&app, entry_id, |a| a == InlineAction::CollapseEntry)
        .expect("bottom collapse button");
    click(&mut app, col_x, col_y);
    assert!(matches!(
        app.transcript.entries[0].kind,
        crate::transcript::EntryKind::Tool { expanded: false, .. }
    ));
    assert!(!app.transcript.is_read_full_file(entry_id));

    let _ = std::fs::remove_file(tmp_path);
}

/// 验证弹窗底栏提示按钮（`Enter 确认`、`Esc/点外部 关闭`）坐标精确对齐、悬停仅前景高亮（无背景）且单击生效。
#[test]
fn picker_footer_hint_buttons_align_hover_without_bg_and_trigger_actions() {
    use crate::ui::OverlayFooterAction;
    use crate::ui::overlay::PickerKind;

    let (mut app, mut term) = make_app(110, 32);
    app.transcript.session_id = Some("121fe1e7-7919-4e98-983d-03aa2520051e".into());
    app.transcript.context_used = 319_500;
    app.transcript.context_window = 1_000_000;
    app.transcript.push_user("测试底栏提示按钮".into(), Vec::new());

    // 1. 点击底栏用量格打开「上下文与会话状态」弹窗
    term.draw(|f| app.render(f)).unwrap();
    let usage = app.last_hits.usage_cell;
    click(&mut app, usage.x + 1, usage.y);
    assert!(app.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Status));
    term.draw(|f| app.render(f)).unwrap();

    let confirm_rect = app
        .last_hits
        .overlay
        .picker
        .footer_actions
        .iter()
        .find(|(_, a)| *a == OverlayFooterAction::Confirm)
        .map(|(r, _)| *r)
        .expect("Confirm footer button hitbox");
    let close_rect = app
        .last_hits
        .overlay
        .picker
        .footer_actions
        .iter()
        .find(|(_, a)| *a == OverlayFooterAction::Close)
        .map(|(r, _)| *r)
        .expect("Close footer button hitbox");

    // 2. 校验热区精确落在边框渲染文字「Enter 确认」和「Esc/点外部 关闭」的起始字符上
    let buf = term.backend().buffer();
    assert_eq!(buf[(confirm_rect.x, confirm_rect.y)].symbol(), "E");
    assert_eq!(buf[(close_rect.x, close_rect.y)].symbol(), "E");

    // 3. 悬停「Esc/点外部 关闭」：仅高亮前景文字，背景保持面板底色不铺色块
    let before_bg = buf[(close_rect.x, close_rect.y)].bg;
    assert!(send_mouse(
        &mut app,
        MouseEventKind::Moved,
        close_rect.x + 1,
        close_rect.y
    ));
    assert_eq!(
        app.hover,
        HoverTarget::OverlayAction(OverlayFooterAction::Close)
    );
    term.draw(|f| app.render(f)).unwrap();
    let after_cell = &term.backend().buffer()[(close_rect.x, close_rect.y)];
    assert_eq!(after_cell.bg, before_bg, "footer button hover must not paint background");
    assert_eq!(
        after_cell.fg,
        theme::accent_bold()
            .fg
            .unwrap_or(theme::palette().accent)
    );

    // 4. 点击「Esc/点外部 关闭」直接关闭弹窗
    click(&mut app, close_rect.x + 1, close_rect.y);
    assert!(app.picker.is_none(), "clicking Close hint button should dismiss picker");

    // 5. 再次打开并选中第 3 项，点击「Enter 确认」直接执行复制会话 ID
    click(&mut app, usage.x + 1, usage.y);
    app.picker.as_mut().unwrap().selected = 2;
    term.draw(|f| app.render(f)).unwrap();
    let confirm_rect = app
        .last_hits
        .overlay
        .picker
        .footer_actions
        .iter()
        .find(|(_, a)| *a == OverlayFooterAction::Confirm)
        .map(|(r, _)| *r)
        .unwrap();
    click(&mut app, confirm_rect.x + 1, confirm_rect.y);
    assert!(app.picker.is_none());
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|(m, _, _)| m.contains("已复制会话 ID")),
        "clicking Enter Confirm button should execute current picker choice"
    );
}

/// 验证 Markdown 正文与行内代码中的 Windows / Linux 文件路径均自动分层着色并绑定点击链接。
#[test]
fn markdown_renders_windows_and_linux_paths_with_links_and_layered_colors() {
    theme::init(ThemePreset::MiyuLavender, true);
    let sample = r"检查 D:\Code\Systemui-dev\Foo.kt:45-51 与 /home/snemc/workspace/src/main.rs 以及 `src/ui/overlay.rs` 的改动。";
    let lines = crate::markdown::render(sample, 100);
    let all_links: Vec<&str> = lines
        .iter()
        .flat_map(|l| l.folded.links.iter().map(|(_, _, u)| u.as_str()))
        .collect();
    assert!(
        all_links.contains(&r"D:\Code\Systemui-dev\Foo.kt"),
        "missing Windows path link: {all_links:?}"
    );
    assert!(
        all_links.contains(&"/home/snemc/workspace/src/main.rs"),
        "missing Linux path link: {all_links:?}"
    );
    assert!(
        all_links.contains(&"src/ui/overlay.rs"),
        "missing inline code path link: {all_links:?}"
    );
}

/// 验证执行工具展开时，短单行命令与多行命令的首行均只展示一次，绝不重复。
#[test]
fn execute_tool_expanded_never_duplicates_command_first_line() {
    use crate::ui::rows::{MdCache, build_body};

    let (mut app, _) = make_app(100, 32);
    let short_id = app.transcript.upsert_tool(
        "exec_short".into(),
        Some("run_command".into()),
        None,
        Some("execute".into()),
        Some("completed".into()),
        Some(json!({ "CommandLine": "cargo test --quiet" })),
        Some("test result: ok".into()),
        Vec::new(),
    );
    let multi_id = app.transcript.upsert_tool(
        "exec_multi".into(),
        Some("run_command".into()),
        None,
        Some("execute".into()),
        Some("completed".into()),
        Some(json!({ "CommandLine": "echo first_unique_line\necho second_line\necho third_line" })),
        Some("done".into()),
        Vec::new(),
    );
    app.transcript.set_entry_expanded(short_id, true);
    app.transcript.set_entry_expanded(multi_id, true);

    let mut cache = MdCache::new();
    let built = build_body(&app.transcript, &[], 96, false, 0, &mut cache);
    let rendered: String = built
        .rows
        .iter()
        .map(|r| r.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        rendered.matches("cargo test --quiet").count(),
        1,
        "short command should appear only once when expanded:\n{rendered}"
    );
    assert_eq!(
        rendered.matches("first_unique_line").count(),
        1,
        "multiline command first line should appear only once when expanded:\n{rendered}"
    );
    assert!(
        rendered.contains("3 行命令"),
        "multiline expanded header should summarize line count:\n{rendered}"
    );
}

/// 验证主视图顶部黏性上条用户消息条在滚出视口时单行展示、悬停前景高亮、点击后跳转回完整消息。
#[test]
fn sticky_top_user_prompt_appears_when_scrolled_past_and_jumps_on_click() {
    let (mut app, mut term) = make_app(100, 24);
    app.transcript.push_user(
        "第一行用户提问摘要\n第二行详细补充说明".into(),
        Vec::new(),
    );
    let prompt_id = app.transcript.entries[0].id;
    let long_reply: String = (1..=40)
        .map(|i| format!("回答正文第 {i:02} 行内容"))
        .collect::<Vec<_>>()
        .join("\n\n");
    app.transcript.append_assistant(&long_reply);

    // 1. 位于顶部时，用户消息仍在视口内，不重复渲染黏性条
    app.auto_follow = false;
    app.scroll = 0;
    term.draw(|f| app.render(f)).unwrap();
    assert!(
        app.last_hits.sticky_prompt.entry_id.is_none(),
        "sticky bar should stay hidden when user prompt is visible at top"
    );

    // 2. 滚到底部后，主视图顶部展示单行黏性用户消息条，悬停仅前景高亮无背景
    app.auto_follow = true;
    term.draw(|f| app.render(f)).unwrap();
    assert_eq!(app.last_hits.sticky_prompt.entry_id, Some(prompt_id));
    let sticky_area = app.last_hits.sticky_prompt.area;
    assert_eq!(sticky_area.y, 1, "sticky prompt should keep 1 row top margin");
    assert!(sticky_area.width > 0 && sticky_area.height == 1);
    let sticky_row = app
        .last_hits
        .screen_snapshot
        .slice_row_text(sticky_area.y, sticky_area.x, sticky_area.right());
    assert!(
        sticky_row.contains("第一行用户提问摘要") && sticky_row.contains("↑ 跳转"),
        "unexpected sticky prompt row: {sticky_row:?}"
    );

    let before_bg = term.backend().buffer()[(sticky_area.x + 2, sticky_area.y)].bg;
    assert!(send_mouse(
        &mut app,
        MouseEventKind::Moved,
        sticky_area.x + 2,
        sticky_area.y
    ));
    assert_eq!(app.hover, HoverTarget::StickyUserPrompt(prompt_id));
    term.draw(|f| app.render(f)).unwrap();
    assert_eq!(
        term.backend().buffer()[(sticky_area.x + 2, sticky_area.y)].bg,
        before_bg
    );

    // 3. 点击黏性条：跳回完整用户消息顶部，重绘后完整多行消息可见且黏性条自动隐去
    click(&mut app, sticky_area.x + 2, sticky_area.y);
    assert!(!app.auto_follow);
    assert_eq!(app.scroll, app.last_hits.entry_starts[&prompt_id]);
    term.draw(|f| app.render(f)).unwrap();
    assert!(app.last_hits.sticky_prompt.entry_id.is_none());
    let top_view: String = (0..12)
        .map(|r| app.last_hits.screen_snapshot.slice_row_text(r, 0, 100))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        top_view.contains("第一行用户提问摘要") && top_view.contains("第二行详细补充说明"),
        "clicking sticky bar should jump to and reveal full user message:\n{top_view}"
    );

    // 4. 追加第二轮对话，从第 1 行逐行滚动至最底部（含跨回合分隔空行），黏性条全程绝不闪烁消失
    app.transcript.push_user("第二轮用户问题".into(), Vec::new());
    let prompt2_id = app.transcript.entries.last().unwrap().id;
    let long_reply2: String = (1..=25)
        .map(|i| format!("第二轮回答第 {i:02} 行"))
        .collect::<Vec<_>>()
        .join("\n\n");
    app.transcript.append_assistant(&long_reply2);
    term.draw(|f| app.render(f)).unwrap();
    let p2_start = app.last_hits.entry_starts[&prompt2_id];
    let max_scroll = app
        .last_hits
        .total_body_rows
        .saturating_sub(app.last_hits.visible_body_rows);
    for s in 1..=max_scroll {
        app.auto_follow = false;
        app.scroll = s;
        term.draw(|f| app.render(f)).unwrap();
        let expected = if s <= p2_start { prompt_id } else { prompt2_id };
        assert_eq!(
            app.last_hits.sticky_prompt.entry_id,
            Some(expected),
            "sticky prompt must never vanish while scrolling at row {s} (p2_start={p2_start})"
        );
    }
}
