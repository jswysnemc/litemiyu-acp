//! litemiyu-acp：参考 LiteMiyu 美学与渲染引擎打造的超轻量 ACP v1 终端客户端。
//! 默认直连 `agy acp`，亦可通过 `--cmd` 对接任意标准 ACP v1 Server。

mod acp;
mod app;
mod clipboard;
#[cfg(all(unix, not(target_os = "macos")))]
mod clipboard_unix;
mod diff;
mod editor;
mod input;
mod keyboard;
mod linebreak;
mod markdown;
mod paste;
mod paths;
mod theme;
mod transcript;
mod ui;

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
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

use crate::acp::{AcpClient, AcpCommand, AcpEvent};
use crate::app::{App, UserPrefs};
use crate::theme::ThemePreset;

#[derive(Parser, Debug)]
#[command(
    name = "litemiyu-acp",
    version,
    about = "LiteMiyu 风格的高颜值 ACP v1 终端前端（默认连接 agy acp）"
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

    /// 指定主题（miyu-lavender / miyu-night / miyu-forest / miyu-day / terminal）
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

    let preset = cli
        .theme
        .as_deref()
        .and_then(ThemePreset::parse)
        .or_else(|| ThemePreset::parse(&prefs.theme))
        .unwrap_or(ThemePreset::MiyuLavender);
    let paint_bg = if cli.transparent || preset == ThemePreset::Terminal {
        false
    } else {
        prefs.paint_bg
    };
    theme::init(preset, paint_bg);

    let cwd = cli
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .to_string_lossy()
        .into_owned();

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AcpEvent>();
    let client = if cli.demo {
        None
    } else {
        let (program, args) = parse_command(&cli.cmd);
        Some(AcpClient::spawn(program, args, cwd.clone(), event_tx)?)
    };

    let show_sidebar = if cli.no_sidebar {
        false
    } else {
        prefs.show_sidebar
    };
    let mut app = App::new(client, cwd, cli.resume, show_sidebar, cli.demo);

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
        client.send(AcpCommand::Shutdown);
    }
    let _ = restore_terminal();
    result
}

/// 非阻塞事件排空超时：
/// 在 `crossterm` 的 `use-dev-tty` Unix 后端中，`Duration::ZERO` 会被 `while !t.is_zero()` 直接跳过；
/// 使用 `200µs` 既不会在构造 `PollTimeout` 到进入循环的极短时间内过期，又在底层 `filedescriptor::poll`
/// 中按毫秒截断为 `0ms` 纯非阻塞。
const NONBLOCK_POLL_TIMEOUT: Duration = Duration::from_micros(200);

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
    let mut last_tick = std::time::Instant::now();
    let mut dirty = true;

    loop {
        if app.external_editor_pending {
            app.external_editor_pending = false;
            let initial = app.prepare_external_editor_buffer();
            let _ = restore_terminal();
            let edited = editor::edit_in_external_editor(&initial).map_err(|e| e.to_string());
            resume_terminal(app.mouse_capture)?;
            terminal.clear()?;
            // 丢弃退出编辑器时残留在终端缓冲区的按键事件，防止误触发发送
            while poll(NONBLOCK_POLL_TIMEOUT).unwrap_or(false) {
                let _ = read();
            }
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

        // 仅在有状态变化、动效进行中或存在定时消失的 Toast 时重绘，空闲零空转
        if dirty || app.busy || app.connecting || app.toast.is_some() {
            draw_frame(terminal, app)?;
            dirty = false;
        }

        if app.should_quit {
            break;
        }

        // 轮询终端输入（动效期 32ms，空闲期 25ms 保证低延迟响应且空闲不重绘）
        let timeout = if app.busy || app.connecting {
            Duration::from_millis(32)
        } else {
            Duration::from_millis(25)
        };
        if poll(timeout)? {
            // 批量收集同一帧内积压的输入，排空内核 tty 与内部解析队列，避免滚轮/快打延迟堆积
            let mut batch = Vec::with_capacity(16);
            batch.push(read()?);
            while batch.len() < 512 && poll(NONBLOCK_POLL_TIMEOUT)? {
                batch.push(read()?);
            }
            for i in 0..batch.len() {
                let has_more_in_batch = batch[i + 1..].iter().any(|ev| {
                    matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Release)
                });
                let has_more_keys = has_more_in_batch
                    || (i + 1 == batch.len() && poll(NONBLOCK_POLL_TIMEOUT).unwrap_or(false));
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

        while let Ok(ev) = event_rx.try_recv() {
            app.handle_acp_event(ev);
            dirty = true;
        }

        if last_tick.elapsed() >= Duration::from_millis(80) {
            app.tick = app.tick.wrapping_add(1);
            last_tick = std::time::Instant::now();
        }
    }
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
