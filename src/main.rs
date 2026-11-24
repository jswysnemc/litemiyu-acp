//! litemiyu-acp：参考 LiteMiyu 美学与渲染引擎打造的超轻量 ACP v1 终端客户端。
//! 默认直连 `agy acp`，亦可通过 `--cmd` 对接任意标准 ACP v1 Server。

mod acp;
mod app;
mod clipboard;
mod diff;
mod input;
mod linebreak;
mod markdown;
mod paste;
mod theme;
mod transcript;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind, poll, read,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
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
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, &mut app, &mut event_rx).await;

    if let Some(client) = &app.client {
        client.send(AcpCommand::Shutdown);
    }
    let _ = restore_terminal();
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<()> {
    let mut last_tick = std::time::Instant::now();

    loop {
        if let Some(enable) = app.mouse_toggle_pending.take() {
            if enable {
                let _ = execute!(io::stdout(), EnableMouseCapture);
            } else {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
        }

        terminal.draw(|f| app.render(f))?;

        if app.should_quit {
            break;
        }

        // 先非阻塞清空全部已到的 ACP 事件
        while let Ok(ev) = event_rx.try_recv() {
            app.handle_acp_event(ev);
        }

        // 轮询终端输入（32ms 统一动效节拍，流光与表情眨眼丝滑且空闲 CPU 极低）
        let timeout = if app.busy || app.connecting {
            Duration::from_millis(32)
        } else {
            Duration::from_millis(80)
        };
        if poll(timeout)? {
            // 批量收集同一帧内积压的输入，便于 Windows 无括号多行粘贴识别与顺滑滚轮
            let mut batch = Vec::with_capacity(16);
            batch.push(read()?);
            while batch.len() < 512 && poll(Duration::from_millis(0))? {
                batch.push(read()?);
            }
            for i in 0..batch.len() {
                let has_more_in_batch = batch[i + 1..].iter().any(|ev| {
                    matches!(ev, Event::Key(k) if k.kind != KeyEventKind::Release)
                });
                let has_more_keys = has_more_in_batch
                    || (i + 1 == batch.len() && poll(Duration::from_millis(0)).unwrap_or(false));
                match &batch[i] {
                    Event::Key(key) => app.handle_key_queued(*key, has_more_keys),
                    Event::Paste(text) => app.handle_paste(text.clone()),
                    Event::Mouse(mouse) => app.handle_mouse(*mouse),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            app.handle_acp_event(ev);
        }

        if last_tick.elapsed() >= Duration::from_millis(80) {
            app.tick = app.tick.wrapping_add(1);
            last_tick = std::time::Instant::now();
        }
    }
    Ok(())
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
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
