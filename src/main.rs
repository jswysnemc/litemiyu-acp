//! agy-tui：面向 Antigravity CLI (`agy`) 打造的超轻量终端 TUI 前端，附带兼容标准 ACP v1 后端。
//! 默认直连 `agy acp`，亦可通过 `--cmd` 对接其他支持 ACP v1 的 Agent Server。

mod acp;
mod app;
mod background;
mod clipboard;
#[cfg(all(unix, not(target_os = "macos")))]
mod clipboard_unix;
mod completion;
mod diff;
mod editor;
mod input;
mod keyboard;
mod linebreak;
mod markdown;
mod paste;
mod paths;
mod subagent;
mod theme;
mod todo;
mod transcript;
mod ui;

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::cursor;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind, poll, read,
};
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::acp::{AcpClient, AcpEvent};
use crate::app::{App, UserPrefs};

#[derive(Parser, Debug)]
#[command(
    name = "agy-tui",
    version,
    about = "面向 Antigravity CLI (agy) 的高颜值轻量终端前端（附带兼容其他 ACP v1 后端）"
)]
struct Cli {
    /// ACP 后端命令（默认："agy acp"；支持附带参数如 "agy acp --sandbox"）
    #[arg(long, default_value = "agy acp")]
    cmd: String,

    /// 会话工作目录（默认当前目录）
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// 启动时直接恢复指定会话 ID
    #[arg(short, long)]
    resume: Option<String>,

    /// 指定内置或自定义主题 ID（如 catppuccin-mocha / rose-pine / kanagawa-wave / nord / miyu-paper / terminal）
    #[arg(long)]
    theme: Option<String>,

    /// 透明背景（继承宿主终端底色，不铺主题底色）
    #[arg(long)]
    transparent: bool,

    /// 启动时隐藏右侧边栏
    #[arg(long)]
    no_sidebar: bool,

    /// 离线预览渲染效果（不启动后端，直接展示复杂表格/代码块/思考折叠样例）
    #[arg(long)]
    demo: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let prefs = UserPrefs::load();

    let requested_theme = cli.theme.as_deref().unwrap_or(&prefs.theme);
    let paint_bg = if cli.transparent { false } else { prefs.paint_bg };
    theme::init_by_id(requested_theme, paint_bg);

    let cwd = cli
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .to_string_lossy()
        .into_owned();

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AcpEvent>();
    let (program, args) = parse_command(&cli.cmd);
    let local_agy = !cli.demo && args.first().is_some_and(|arg| arg == "acp")
        && std::path::Path::new(&program).file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("agy") || name.eq_ignore_ascii_case("agy.exe"));
    let client = if cli.demo {
        None
    } else {
        Some(AcpClient::spawn(program, args, cwd.clone(), event_tx)?)
    };

    let show_sidebar = if cli.no_sidebar {
        false
    } else {
        prefs.show_sidebar
    };
    let mut app = App::new(client, cwd, cli.resume, show_sidebar, cli.demo);
    app.queued_mode = prefs.queued_mode;
    if local_agy && let Some(root) = paths::agy_brain_dir() { app.subagents.enable_local(root); }

    // 终端初始化与 panic 还原保护
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        orig_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    keyboard::enable(&mut stdout);
    // 使用 64KB 缓冲包裹 stdout，配合 DEC Mode 2026 同步更新，保证整帧原子提交不撕裂
    let backend = CrosstermBackend::new(BufWriter::with_capacity(64 * 1024, stdout));
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, &mut app, &mut event_rx).await;

    if let Some(client) = &app.client {
        client.shutdown().await;
    }
    let _ = restore_terminal();
    result
}

/// 非阻塞事件排空超时：
/// 在 `crossterm` 的 `use-dev-tty` Unix 后端中，`Duration::ZERO` 会被 `while !t.is_zero()` 直接跳过；
/// 使用 `200µs` 既不会在构造 `PollTimeout` 到进入循环的极短时间内过期，又在底层 `filedescriptor::poll`
/// 中按毫秒截断为 `0ms` 纯非阻塞。
const NONBLOCK_POLL_TIMEOUT: Duration = Duration::from_micros(200);

/// 独立终端输入读取线程控制器（支持外部编辑器期间暂停释放 `/dev/tty` 与退出时优雅停机）。
struct TerminalInputControl {
    stop: AtomicBool,
    pause_requested: AtomicBool,
    paused: Mutex<bool>,
    cv: Condvar,
}

struct TerminalInputReader {
    ctrl: Arc<TerminalInputControl>,
    rx: mpsc::UnboundedReceiver<Event>,
}

impl TerminalInputReader {
    /// 启动后台阻塞输入线程，将终端事件转发至异步通道供 `tokio::select!` 即时消费。
    ///
    /// 返回:
    /// - 包含事件接收端与暂停控制器的 `TerminalInputReader`
    fn spawn() -> Self {
        let ctrl = Arc::new(TerminalInputControl {
            stop: AtomicBool::new(false),
            pause_requested: AtomicBool::new(false),
            paused: Mutex::new(false),
            cv: Condvar::new(),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let worker_ctrl = Arc::clone(&ctrl);

        std::thread::spawn(move || {
            loop {
                if worker_ctrl.stop.load(Ordering::Acquire) {
                    break;
                }
                // 1. 外部编辑器需要独占 /dev/tty 时，先确认暂停并等待恢复信号
                if worker_ctrl.pause_requested.load(Ordering::Acquire) {
                    let mut guard = worker_ctrl
                        .paused
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *guard = true;
                    worker_ctrl.cv.notify_all();
                    while worker_ctrl.pause_requested.load(Ordering::Acquire)
                        && !worker_ctrl.stop.load(Ordering::Acquire)
                    {
                        guard = worker_ctrl
                            .cv
                            .wait(guard)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    *guard = false;
                    continue;
                }
                // 2. 以 40ms 轮询终端事件，收到事件后立即排空积压批量送入异步通道
                match poll(Duration::from_millis(40)) {
                    Ok(true) => {
                        if worker_ctrl.pause_requested.load(Ordering::Acquire)
                            || worker_ctrl.stop.load(Ordering::Acquire)
                        {
                            continue;
                        }
                        if let Ok(ev) = read() {
                            if tx.send(ev).is_err() {
                                break;
                            }
                            while !worker_ctrl.pause_requested.load(Ordering::Acquire)
                                && !worker_ctrl.stop.load(Ordering::Acquire)
                                && poll(NONBLOCK_POLL_TIMEOUT).unwrap_or(false)
                            {
                                match read() {
                                    Ok(next_ev) => {
                                        if tx.send(next_ev).is_err() {
                                            return;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                    Ok(false) => {}
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        });

        Self { ctrl, rx }
    }

    /// 暂停输入线程并同步等待其彻底停止读取 `/dev/tty`（供 `$EDITOR` 外部编辑器安全接管终端）。
    fn pause(&mut self) {
        self.ctrl.pause_requested.store(true, Ordering::Release);
        let mut guard = self
            .ctrl
            .paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*guard {
            guard = self
                .ctrl
                .cv
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        while self.rx.try_recv().is_ok() {}
    }

    /// 恢复输入线程继续读取终端事件（外部编辑器退出后调用）。
    fn resume(&mut self) {
        while self.rx.try_recv().is_ok() {}
        let _guard = self
            .ctrl
            .paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ctrl.pause_requested.store(false, Ordering::Release);
        self.ctrl.cv.notify_all();
    }

    /// 通知输入线程退出。
    fn stop(&self) {
        self.ctrl.stop.store(true, Ordering::Release);
        self.ctrl.pause_requested.store(false, Ordering::Release);
        self.ctrl.cv.notify_all();
    }
}

/// 使用 DEC Mode 2026 同步输出 + 帧内隐藏光标完成原子重绘，避免终端硬件光标在正文与输入框之间跳动。
///
/// 参数:
/// - `terminal`: 带缓冲的终端实例
/// - `app`: 应用状态机
///
/// 返回:
/// - 绘制或终端写入失败时返回错误
fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<io::Stdout>>>,
    app: &mut App,
) -> Result<()> {
    // 1. 挂起终端同步刷新并先隐藏硬件光标，防止写入正文差异时暴露光标移动轨迹
    queue!(
        terminal.backend_mut(),
        BeginSynchronizedUpdate,
        cursor::Hide
    )?;
    // 2. 执行 ratatui 整帧差异渲染（内部在末尾将光标定位到输入框并恢复显示）
    let draw_res = terminal.draw(|f| app.render(f)).map(|_| ());
    // 3. 无论绘制是否出错，都必须关闭同步更新并冲刷缓冲，防止终端画面挂起
    let end_res = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    draw_res?;
    end_res?;
    Ok(())
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<io::Stdout>>>,
    app: &mut App,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<()> {
    let mut term_input = TerminalInputReader::spawn();
    let mut last_tick = std::time::Instant::now();
    let mut last_bg_draw = std::time::Instant::now();
    let mut dirty = true;

    loop {
        if app.external_editor_pending {
            app.external_editor_pending = false;
            let initial = app.prepare_external_editor_buffer();
            term_input.pause();
            let _ = restore_terminal();
            let edited = editor::edit_in_external_editor(&initial).map_err(|e| e.to_string());
            resume_terminal(app.mouse_capture)?;
            terminal.clear()?;
            // 丢弃退出编辑器时残留在终端缓冲区的按键事件，防止误触发发送
            while poll(NONBLOCK_POLL_TIMEOUT).unwrap_or(false) {
                let _ = read();
            }
            term_input.resume();
            app.apply_external_editor_result(edited);
            dirty = true;
        }

        if let Some(enable) = app.mouse_toggle_pending.take() {
            if enable {
                let _ = execute!(io::stdout(), EnableMouseCapture);
            } else {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
            dirty = true;
        }

        // 先非阻塞清空全部已到的 ACP 事件
        while let Ok(ev) = event_rx.try_recv() {
            app.handle_acp_event(ev);
            dirty = true;
        }

        if app.tick_timers() {
            dirty = true;
        }

        if last_tick.elapsed() >= Duration::from_millis(80) {
            app.tick = app.tick.wrapping_add(1);
            last_tick = std::time::Instant::now();
        }

        // 仅在有状态变化、动效进行中或存在后台任务节拍时重绘，空闲零空转
        let animating = app.is_generating() || app.connecting;
        let bg_tick_due = app.has_background_tasks()
            && last_bg_draw.elapsed() >= Duration::from_millis(250);
        if dirty || animating || bg_tick_due {
            draw_frame(terminal, app)?;
            dirty = false;
            if bg_tick_due {
                last_bg_draw = std::time::Instant::now();
            }
        }

        if app.should_quit {
            break;
        }

        // 事件驱动等待：同时监听终端输入、ACP 异步流与精确计算的定时唤醒（彻底空闲时 0 唤醒）
        let mut wait_delay = app.next_timer_delay();
        if animating {
            let d = Duration::from_millis(32);
            wait_delay = Some(wait_delay.map_or(d, |cur| cur.min(d)));
        } else if app.has_background_tasks() {
            let rem = Duration::from_millis(250).saturating_sub(last_bg_draw.elapsed());
            wait_delay = Some(wait_delay.map_or(rem, |cur| cur.min(rem)));
        }

        tokio::select! {
            maybe_term = term_input.rx.recv() => {
                let Some(first_ev) = maybe_term else {
                    break;
                };
                let mut batch = Vec::with_capacity(16);
                batch.push(first_ev);
                while batch.len() < 512 {
                    match term_input.rx.try_recv() {
                        Ok(ev) => batch.push(ev),
                        Err(_) => break,
                    }
                }
                for i in 0..batch.len() {
                    let has_more_in_batch = batch[i + 1..].iter().any(|ev| {
                        matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Release)
                    });
                    let has_more_keys = has_more_in_batch || !term_input.rx.is_empty();
                    match &batch[i] {
                        Event::Key(key) => {
                            app.handle_key_queued(*key, has_more_keys);
                            dirty = true;
                        }
                        Event::Paste(text) => {
                            app.handle_paste(text.clone());
                            dirty = true;
                        }
                        Event::Mouse(mouse) => {
                            if app.handle_mouse(*mouse) {
                                dirty = true;
                            }
                        }
                        Event::Resize(_, _) => {
                            dirty = true;
                        }
                        _ => {}
                    }
                }
            }
            maybe_acp = event_rx.recv() => {
                if let Some(ev) = maybe_acp {
                    app.handle_acp_event(ev);
                    dirty = true;
                    while let Ok(next_ev) = event_rx.try_recv() {
                        app.handle_acp_event(next_ev);
                    }
                }
            }
            () = async {
                match wait_delay {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    term_input.stop();
    Ok(())
}

fn restore_terminal() -> Result<()> {
    let mut stdout = io::stdout();
    // 1. 必须在离开备用屏之前弹出 Kitty 键盘增强协议，并解除同步更新与光标隐藏
    keyboard::disable(&mut stdout);
    execute!(
        stdout,
        EndSynchronizedUpdate,
        cursor::Show,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    let _ = stdout.flush();
    disable_raw_mode()?;
    Ok(())
}

fn resume_terminal(mouse_capture: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse_capture {
        execute!(stdout, EnableMouseCapture)?;
    }
    keyboard::enable(&mut stdout);
    Ok(())
}

fn parse_command(raw: &str) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in raw.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, q @ ('"' | '\'')) => quote = Some(q),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    if parts.is_empty() {
        return ("agy".into(), vec!["acp".into()]);
    }
    let program = parts.remove(0);
    #[cfg(windows)]
    let program = if program.eq_ignore_ascii_case("agy") {
        "agy.exe".into()
    } else {
        program
    };
    (program, parts)
}
