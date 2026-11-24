//! 终端键盘增强协议（Kitty Keyboard Protocol）探测与生命周期管理。
//!
//! Linux / macOS 下的 Kitty、Ghostty、WezTerm 等终端在默认传统模式中，
//! 会将 `Shift+Enter` 编码为与普通 `Enter` 相同的 `\r` (0x0D)，导致直接提交消息。
//! 本模块在进入备用屏后按需探测并压入 `DISAMBIGUATE_ESCAPE_CODES`，退出备用屏前成对弹出。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(windows))]
use std::sync::OnceLock;

#[cfg(not(windows))]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
#[cfg(not(windows))]
use crossterm::execute;

static ENHANCEMENT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 探测当前终端是否支持 Kitty 键盘增强协议（进程内仅探测一次）。
///
/// 返回:
/// - 终端明确支持协议时返回 `true`，Windows 与不支持的终端返回 `false`
#[cfg(not(windows))]
fn supports_enhancement() -> bool {
    if cfg!(test) {
        return false;
    }
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false))
}

/// 在已开启 raw mode 且进入备用屏后，按需压入 Kitty 键盘增强协议。
///
/// 参数:
/// - `writer`: 终端输出流（通常为 stdout）
///
/// 返回:
/// - 实际压入协议栈时返回 `true`，跳过或失败时返回 `false`
pub fn enable<W: Write>(writer: &mut W) -> bool {
    #[cfg(windows)]
    {
        // Windows 控制台事件原生区分 Shift+Enter，无需压入 Kitty 协议序列
        let _ = writer;
        false
    }
    #[cfg(not(windows))]
    {
        // 1. 若当前已在增强态或终端不支持，则直接跳过
        if ENHANCEMENT_ACTIVE.load(Ordering::SeqCst) || !supports_enhancement() {
            return false;
        }
        // 2. 仅压入 Level 1 消歧义标志，精准区分 Shift+Enter / Ctrl+Enter / Alt+Enter 且不影响普通字符
        if execute!(
            writer,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
        {
            ENHANCEMENT_ACTIVE.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }
}

/// 在离开备用屏前弹出已压入的 Kitty 键盘增强协议，避免残留到主屏影响 Shell。
///
/// 参数:
/// - `writer`: 终端输出流（通常为 stdout）
///
/// 返回:
/// - 无
pub fn disable<W: Write>(writer: &mut W) {
    #[cfg(windows)]
    {
        let _ = writer;
    }
    #[cfg(not(windows))]
    {
        if ENHANCEMENT_ACTIVE.swap(false, Ordering::SeqCst) {
            let _ = execute!(writer, PopKeyboardEnhancementFlags);
        }
    }
}

/// 查询当前是否处于键盘增强协议激活状态（供测试与状态诊断使用）。
///
/// 返回:
/// - 已压入且尚未弹出时返回 `true`
#[cfg(test)]
pub fn is_active() -> bool {
    ENHANCEMENT_ACTIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_is_noop_when_not_enabled() {
        let mut buf = Vec::new();
        assert!(!is_active());
        disable(&mut buf);
        assert!(buf.is_empty());
        assert!(!is_active());
    }

    #[test]
    fn enable_skips_probing_in_unit_tests() {
        let mut buf = Vec::new();
        assert!(!enable(&mut buf));
        assert!(buf.is_empty());
        assert!(!is_active());
    }
}
