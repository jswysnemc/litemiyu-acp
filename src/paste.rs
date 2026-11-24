//! Windows 无括号多行粘贴检测、ConPTY CRLF 配对吸收与原生控制台回放批量消费
//!（完整参考 `jswysnemc/sai` 的 `repl_windows_paste/` 与 `repl_input/paste.rs`）。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

const CHAR_INTERVAL: Duration = Duration::from_millis(15);
const CONTROL_SUPPRESS_WINDOW: Duration = Duration::from_millis(160);
const MIN_BURST_CHARS: usize = 3;
const CLIPBOARD_MATCH_WINDOW: Duration = Duration::from_millis(250);
const CRLF_PAIR_WINDOW: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasteKey {
    Char(char),
    Enter,
    LineFeed,
    Tab,
}

#[derive(Debug, Eq, PartialEq)]
pub enum WindowsPasteOutcome {
    /// 非粘贴事件，交由正常按键逻辑处理。
    NotPaste,
    /// 已识别多行粘贴的后续回放按键（含 ConPTY CRLF 配对 LF），直接吞并丢弃。
    ConsumedReplay,
    /// 首行与系统剪贴板多行文本精确匹配：删掉已落入编辑器的首行前缀，并整段按粘贴处理。
    ReplacePrefixWithPaste {
        prefix_bytes: usize,
        full_text: String,
    },
    /// 突发输入流中的控制键（回车/Tab），按正文字符插入而非触发发送。
    InsertBurstChar(char),
}

#[derive(Clone, Debug, Default)]
struct PasteBurst {
    last_char_at: Option<Instant>,
    consecutive_chars: usize,
    suppress_until: Option<Instant>,
}

impl PasteBurst {
    fn record_char(&mut self, now: Instant) {
        if !self
            .last_char_at
            .is_some_and(|prev| now.duration_since(prev) <= CHAR_INTERVAL)
        {
            self.consecutive_chars = 0;
        }
        self.last_char_at = Some(now);
        self.consecutive_chars = self.consecutive_chars.saturating_add(1);
        if self.consecutive_chars >= MIN_BURST_CHARS {
            self.suppress_until = Some(now + CONTROL_SUPPRESS_WINDOW);
        }
    }

    fn should_protect_control(&self, now: Instant) -> bool {
        let recent_char = self
            .last_char_at
            .is_some_and(|prev| now.duration_since(prev) <= CHAR_INTERVAL);
        let in_window = self.suppress_until.is_some_and(|until| now <= until);
        recent_char || in_window
    }

    fn consume_control(&mut self, now: Instant) -> bool {
        if !self.should_protect_control(now) {
            return false;
        }
        self.suppress_until = Some(now + CONTROL_SUPPRESS_WINDOW);
        true
    }

    fn extend(&mut self, now: Instant) {
        self.suppress_until = Some(now + CONTROL_SUPPRESS_WINDOW);
    }
}

/// 保留已消费 CR 对应的可选 LF，兼容 Windows ConPTY 将 `\r\n` 拆成 `Enter` + `Ctrl+Enter`/`Ctrl+J` 或合并成单个 `Enter`。
#[derive(Clone, Debug, Default)]
struct PasteControl {
    pending_lf_until: Option<Instant>,
}

impl PasteControl {
    fn record_newline(&mut self, key: PasteKey, crlf: bool, now: Instant) {
        self.pending_lf_until =
            (key == PasteKey::Enter && crlf).then_some(now + CRLF_PAIR_WINDOW);
    }

    fn consume_paired_lf(&mut self, key: PasteKey, now: Instant) -> bool {
        self.consume_pair(key, now, false)
    }

    fn consume_pair(&mut self, key: PasteKey, now: Instant, replaying: bool) -> bool {
        let pending = self.pending_lf_until.take();
        key == PasteKey::LineFeed && pending.is_some_and(|until| replaying || now <= until)
    }

    fn has_pending_lf(&self, now: Instant) -> bool {
        self.pending_lf_until.is_some_and(|until| now <= until)
    }

    fn matching_bytes(&mut self, text: &str, key: PasteKey, now: Instant) -> Option<usize> {
        // CRLF 的第二个 LF 事件不推进正文，也不能落入提交分支
        if self.consume_pair(key, now, !text.is_empty()) {
            return Some(0);
        }
        match key {
            PasteKey::Enter | PasteKey::LineFeed => {
                let bytes = newline_bytes(text)?;
                self.record_newline(key, bytes == 2, now);
                Some(bytes)
            }
            PasteKey::Char(ch) => text.starts_with(ch).then_some(ch.len_utf8()),
            PasteKey::Tab => text.starts_with('\t').then_some(1),
        }
    }
}

fn newline_bytes(text: &str) -> Option<usize> {
    if text.starts_with("\r\n") {
        Some(2)
    } else if text.starts_with(['\r', '\n']) {
        Some(1)
    } else {
        None
    }
}

/// 对齐 `sai/src/cli/repl_input/event_batch.rs::windows_paste_key`：
/// 关键点：Windows ConPTY 常把 `\n` (LF) 编码为 `KeyCode::Enter + CONTROL` 或 `KeyCode::Char('j') + CONTROL` 或 `Char('\n')`。
pub fn classify_key(key: &KeyEvent) -> Option<PasteKey> {
    let modifiers = key.modifiers;
    match key.code {
        KeyCode::Enter | KeyCode::Char('j' | 'J')
            if modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.intersects(
                    KeyModifiers::ALT
                        | KeyModifiers::SUPER
                        | KeyModifiers::HYPER
                        | KeyModifiers::META,
                ) =>
        {
            Some(PasteKey::LineFeed)
        }
        KeyCode::Char('\n') if modifiers.is_empty() || modifiers == KeyModifiers::CONTROL => {
            Some(PasteKey::LineFeed)
        }
        KeyCode::Char('\r') if modifiers.is_empty() => Some(PasteKey::Enter),
        KeyCode::Char(c) if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            Some(PasteKey::Char(c))
        }
        KeyCode::Enter if modifiers.is_empty() => Some(PasteKey::Enter),
        KeyCode::Tab if modifiers.is_empty() => Some(PasteKey::Tab),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
pub struct WindowsPasteState {
    burst: PasteBurst,
    control: PasteControl,
    recent_chars: String,
    last_char_at: Option<Instant>,
    replay: String,
    replay_offset: usize,
    replay_confirmed: bool,
}

impl WindowsPasteState {
    pub fn reset(&mut self) {
        self.burst = PasteBurst::default();
        self.reset_clipboard_candidate();
    }

    fn reset_clipboard_candidate(&mut self) {
        self.recent_chars.clear();
        self.last_char_at = None;
        self.control = PasteControl::default();
        self.replay.clear();
        self.replay_offset = 0;
        self.replay_confirmed = false;
    }

    fn record_char(&mut self, ch: char, now: Instant) {
        self.burst.record_char(now);
        if self
            .last_char_at
            .is_some_and(|prev| now.duration_since(prev) > CLIPBOARD_MATCH_WINDOW)
        {
            self.recent_chars.clear();
        }
        self.last_char_at = Some(now);
        self.recent_chars.push(ch);
        if self.recent_chars.len() > 64 * 1024 {
            let cut = self.recent_chars.len() - 32 * 1024;
            let boundary = self
                .recent_chars
                .char_indices()
                .find(|&(i, _)| i >= cut)
                .map_or(self.recent_chars.len(), |(i, _)| i);
            self.recent_chars.drain(..boundary);
        }
    }

    fn consume_replay(&mut self, key: PasteKey, now: Instant) -> bool {
        if !self.replay_confirmed && !self.control.has_pending_lf(now) {
            return false;
        }
        let finished = self.replay_confirmed && self.replay_offset == self.replay.len();
        let Some(bytes) =
            self.control
                .matching_bytes(&self.replay[self.replay_offset..], key, now)
        else {
            if finished {
                self.burst = PasteBurst::default();
            }
            self.replay.clear();
            self.replay_offset = 0;
            self.replay_confirmed = false;
            self.control = PasteControl::default();
            return false;
        };
        self.replay_offset += bytes;
        if self.replay_offset == self.replay.len() {
            self.replay.clear();
            self.replay_offset = 0;
        }
        self.finish_replay_if_done(now);
        true
    }

    fn finish_replay_if_done(&mut self, now: Instant) {
        if self.replay_confirmed && self.replay.is_empty() && !self.control.has_pending_lf(now) {
            self.replay_confirmed = false;
            self.burst = PasteBurst::default();
        }
    }

    fn begin_multiline_from_text(
        &mut self,
        text_before_cursor: &str,
        clipboard_text: &str,
        key: PasteKey,
        now: Instant,
    ) -> Option<(usize, String)> {
        let normalized = clipboard_text.replace("\r\n", "\n").replace('\r', "\n");
        let sep = normalized.find('\n')?;
        let first_line = &normalized[..sep];
        if first_line.is_empty()
            || !text_before_cursor.ends_with(first_line)
            || !self.recent_chars.ends_with(first_line)
        {
            self.reset_clipboard_candidate();
            return None;
        }
        let raw_sep = clipboard_text.find(['\r', '\n'])?;
        let nl_bytes = newline_bytes(&clipboard_text[raw_sep..])?;
        self.recent_chars.clear();
        self.last_char_at = None;
        self.replay = clipboard_text[raw_sep + nl_bytes..].to_string();
        self.replay_offset = 0;
        self.replay_confirmed = true;
        self.control.record_newline(key, nl_bytes == 2, now);
        self.finish_replay_if_done(now);
        Some((first_line.len(), normalized))
    }

    /// 评估当前键盘事件是否属于无括号多行粘贴（严格对齐 `sai/src/cli/repl_input/paste.rs::handle_key`）。
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        text_before_cursor: &str,
        has_more_queued_keys: bool,
        now: Instant,
        read_clipboard: impl FnOnce() -> Option<String>,
    ) -> WindowsPasteOutcome {
        if key.kind == KeyEventKind::Release {
            return WindowsPasteOutcome::NotPaste;
        }
        // Windows 粘贴大写字符/符号时会穿插裸 Modifier 按下事件，绝不能因此重置粘贴状态！
        if matches!(key.code, KeyCode::Modifier(_)) {
            return WindowsPasteOutcome::NotPaste;
        }
        let Some(pk) = classify_key(&key) else {
            self.reset();
            return WindowsPasteOutcome::NotPaste;
        };

        // 1. 已确认剪贴板回放阶段（或等待 CRLF 配对 LF）：优先匹配并吞并
        if self.replay_confirmed && self.consume_replay(pk, now) {
            return WindowsPasteOutcome::ConsumedReplay;
        }

        // 2. 普通字符：登记突发节奏与候选首行，交由编辑器正常落字
        if let PasteKey::Char(ch) = pk {
            self.record_char(ch, now);
            return WindowsPasteOutcome::NotPaste;
        }

        // 3. 配对 CRLF 的第二个 LF 事件直接吞并，避免重复换行或误提交
        if self.control.consume_paired_lf(pk, now) {
            return WindowsPasteOutcome::ConsumedReplay;
        }

        // 4. 非突发状态下的单独 Ctrl+Enter / Ctrl+J 交还给普通换行逻辑（不读取剪贴板）
        if pk == PasteKey::LineFeed
            && !self.burst.should_protect_control(now)
            && !has_more_queued_keys
        {
            return WindowsPasteOutcome::NotPaste;
        }

        // 5. Enter / LF：先尝试与系统剪贴板多行首行比对（对齐 `sai`：250ms 窗口内且存在候选首行即核对）
        if matches!(pk, PasteKey::Enter | PasteKey::LineFeed) {
            let candidate_recent = self
                .last_char_at
                .is_some_and(|prev| now.duration_since(prev) <= CLIPBOARD_MATCH_WINDOW);
            if candidate_recent
                && (pk == PasteKey::Enter
                    || has_more_queued_keys
                    || self.burst.should_protect_control(now))
            {
                if let Some(clip) = read_clipboard() {
                    if let Some((prefix_bytes, full_text)) =
                        self.begin_multiline_from_text(text_before_cursor, &clip, pk, now)
                    {
                        return WindowsPasteOutcome::ReplacePrefixWithPaste {
                            prefix_bytes,
                            full_text,
                        };
                    }
                }
            }

            // 剪贴板未命中（或首行为空 / 剪贴板不可用），但处于突发输入或队列紧跟后续按键：插入换行而非提交！
            if has_more_queued_keys || self.burst.consume_control(now) {
                self.burst.extend(now);
                self.control.record_newline(pk, true, now);
                return WindowsPasteOutcome::InsertBurstChar('\n');
            }
            return WindowsPasteOutcome::NotPaste;
        }

        // 6. 突发粘贴流中的 Tab 按正文插入并同步记录到候选首行
        if pk == PasteKey::Tab && (has_more_queued_keys || self.burst.consume_control(now)) {
            self.burst.extend(now);
            self.recent_chars.push('\t');
            self.last_char_at = Some(now);
            return WindowsPasteOutcome::InsertBurstChar('\t');
        }

        WindowsPasteOutcome::NotPaste
    }

    /// 批量消费 Windows 控制台输入队列中已确认属于当前粘贴回放的原生事件（参考 `sai/src/cli/repl_windows_paste/native.rs`）。
    pub fn drain_console_replay(&mut self) -> usize {
        #[cfg(windows)]
        {
            native::drain(self).unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            0
        }
    }
}

#[cfg(windows)]
mod replay {
    use super::{PasteControl, PasteKey};
    use std::time::Instant;

    #[derive(Clone, Copy, Debug)]
    pub enum ReplayRecord {
        Release,
        Unit(u16),
        Enter,
        LineFeed,
        Boundary,
    }

    #[derive(Debug, Default, Eq, PartialEq)]
    pub struct MatchedPrefix {
        pub records: usize,
        pub bytes: usize,
    }

    pub fn matching_prefix_with_control(
        text: &str,
        control: &PasteControl,
        now: Instant,
        records: impl IntoIterator<Item = ReplayRecord>,
    ) -> (MatchedPrefix, PasteControl) {
        let mut matched = MatchedPrefix::default();
        let mut control = control.clone();
        let mut low_surrogate = false;
        for (index, record) in records.into_iter().enumerate() {
            let remaining = &text[matched.bytes..];
            if remaining.is_empty() && !control.has_pending_lf(now) {
                break;
            }
            let unit = match record {
                ReplayRecord::Boundary => break,
                ReplayRecord::Release => {
                    if !low_surrogate {
                        matched.records = index + 1;
                    }
                    continue;
                }
                ReplayRecord::Unit(u) => u,
                ReplayRecord::Enter => b'\r' as u16,
                ReplayRecord::LineFeed => b'\n' as u16,
            };
            if !low_surrogate && matches!(unit, 9 | 10 | 13) {
                let key = match unit {
                    9 => PasteKey::Tab,
                    10 => PasteKey::LineFeed,
                    _ => PasteKey::Enter,
                };
                let Some(bytes) = control.matching_bytes(remaining, key, now) else {
                    break;
                };
                matched.bytes += bytes;
                matched.records = index + 1;
                continue;
            }
            let Some(expected) = remaining.chars().next() else {
                break;
            };
            let mut encoded = [0u16; 2];
            let expected_units = expected.encode_utf16(&mut encoded);
            if expected_units[usize::from(low_surrogate)] != unit {
                break;
            }
            if expected_units.len() == 2 && !low_surrogate {
                low_surrogate = true;
                continue;
            }
            low_surrogate = false;
            let _ = control.matching_bytes(remaining, PasteKey::Char(expected), now);
            matched.bytes += expected.len_utf8();
            matched.records = index + 1;
        }
        (matched, control)
    }
}

#[cfg(windows)]
mod native {
    use std::fs::OpenOptions;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::System::Console::{
        INPUT_RECORD, KEY_EVENT, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, PeekConsoleInputW,
        RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, ReadConsoleInputW, SHIFT_PRESSED,
    };

    use super::WindowsPasteState;
    use super::replay::{ReplayRecord, matching_prefix_with_control};

    const BATCH_RECORDS: usize = 2048;
    const MAX_BATCHES: usize = 16;
    const DRAIN_BUDGET: Duration = Duration::from_millis(4);

    pub fn drain(state: &mut WindowsPasteState) -> io::Result<usize> {
        if !state.replay_confirmed
            || (state.replay.is_empty() && !state.control.has_pending_lf(Instant::now()))
        {
            return Ok(0);
        }
        let console = OpenOptions::new().read(true).write(true).open("CONIN$")?;
        let handle = console.as_raw_handle();
        let mut records: [INPUT_RECORD; BATCH_RECORDS] = unsafe { std::mem::zeroed() };
        let started = Instant::now();
        let mut total = 0;

        for _ in 0..MAX_BATCHES {
            let now = Instant::now();
            if (state.replay.is_empty() && !state.control.has_pending_lf(now))
                || started.elapsed() >= DRAIN_BUDGET
            {
                break;
            }
            let mut available = 0u32;
            if unsafe {
                PeekConsoleInputW(handle, records.as_mut_ptr(), BATCH_RECORDS as u32, &mut available)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let (matched, _) = matching_prefix_with_control(
                &state.replay[state.replay_offset..],
                &state.control,
                now,
                records[..available as usize].iter().map(replay_record),
            );
            if matched.records == 0 {
                break;
            }
            let mut consumed = 0u32;
            if unsafe {
                ReadConsoleInputW(
                    handle,
                    records.as_mut_ptr(),
                    matched.records as u32,
                    &mut consumed,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let (confirmed, control) = matching_prefix_with_control(
                &state.replay[state.replay_offset..],
                &state.control,
                now,
                records[..consumed as usize].iter().map(replay_record),
            );
            state.replay_offset += confirmed.bytes;
            state.control = control;
            total += consumed as usize;
            if state.replay_offset == state.replay.len() {
                state.replay.clear();
                state.replay_offset = 0;
            }
            state.finish_replay_if_done(now);
            if consumed as usize != matched.records || confirmed.records != consumed as usize {
                break;
            }
        }
        Ok(total)
    }

    fn replay_record(record: &INPUT_RECORD) -> ReplayRecord {
        if record.EventType != KEY_EVENT as u16 {
            return ReplayRecord::Boundary;
        }
        let key = unsafe { record.Event.KeyEvent };
        if key.bKeyDown == 0 {
            return ReplayRecord::Release;
        }
        let unit = unsafe { key.uChar.UnicodeChar };
        if unit == 0 && matches!(key.wVirtualKeyCode, 0x10..=0x12) {
            return ReplayRecord::Release;
        }
        if unit == 10
            && key.wRepeatCount == 1
            && key.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) == 0
            && matches!(key.wVirtualKeyCode, 0x0d | 0x4a)
        {
            return ReplayRecord::LineFeed;
        }
        if key.wRepeatCount != 1
            || key.dwControlKeyState
                & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED)
                != 0
        {
            return ReplayRecord::Boundary;
        }
        match key.wVirtualKeyCode {
            0x0d | 0x09 if key.dwControlKeyState & SHIFT_PRESSED != 0 => ReplayRecord::Boundary,
            0x0d => ReplayRecord::Enter,
            0x09 => ReplayRecord::Unit(b'\t' as u16),
            0x08 | 0x1b | 0x10..=0x12 | 0x21..=0x2f | 0x70..=0x87 => ReplayRecord::Boundary,
            _ => {
                if unit < 0x20 || unit == 0x7f {
                    ReplayRecord::Boundary
                } else {
                    ReplayRecord::Unit(unit)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputEditor;

    fn replay_conpty_into_editor(text: &str, lf_code: KeyCode, clipboard_available: bool) -> Vec<String> {
        let t0 = Instant::now();
        let mut st = WindowsPasteState::default();
        let mut ed = InputEditor::new();
        let mut submissions = Vec::new();

        for (i, ch) in text.chars().enumerate() {
            let now = t0 + Duration::from_millis(i as u64);
            let ev = match ch {
                '\r' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                '\n' => KeyEvent::new(lf_code, KeyModifiers::CONTROL),
                '\t' => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                other => {
                    // 模拟 Windows 上大写/符号字符前穿插裸 Shift 修饰键按下
                    if other.is_uppercase() || other == '$' {
                        let m = KeyEvent::new(
                            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftShift),
                            KeyModifiers::SHIFT,
                        );
                        assert_eq!(
                            st.handle_key(m, ed.text_before_cursor(), false, now, || None),
                            WindowsPasteOutcome::NotPaste
                        );
                    }
                    KeyEvent::new(KeyCode::Char(other), KeyModifiers::NONE)
                }
            };
            match st.handle_key(ev, ed.text_before_cursor(), false, now, || {
                clipboard_available.then(|| text.to_string())
            }) {
                WindowsPasteOutcome::ConsumedReplay => {}
                WindowsPasteOutcome::ReplacePrefixWithPaste {
                    prefix_bytes,
                    full_text,
                } => {
                    ed.delete_bytes_before_cursor(prefix_bytes);
                    let _ = ed.insert_pasted_text(&full_text);
                }
                WindowsPasteOutcome::InsertBurstChar(c) => ed.insert_char(c),
                WindowsPasteOutcome::NotPaste => match (ev.code, ev.modifiers) {
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        let (_, exp) = ed.take_and_record();
                        submissions.push(exp.trim().to_string());
                    }
                    (KeyCode::Char('j'), m) if m.contains(KeyModifiers::CONTROL) => {
                        ed.insert_char('\n');
                    }
                    (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) => {
                        ed.insert_char('\n');
                    }
                    (KeyCode::Char(c), _) => ed.insert_char(c),
                    other => panic!("unexpected unhandled event during paste: {other:?}"),
                },
            }
            // 同步重放 Windows Release 事件
            let rel = KeyEvent::new_with_kind(ev.code, ev.modifiers, KeyEventKind::Release);
            assert_eq!(
                st.handle_key(rel, ed.text_before_cursor(), false, now, || None),
                WindowsPasteOutcome::NotPaste
            );
        }

        // 粘贴结束后手动回车，应当且仅应当提交一次完整正文
        let end_now = t0 + Duration::from_millis(text.chars().count() as u64 + 400);
        assert_eq!(
            st.handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                ed.text_before_cursor(),
                false,
                end_now,
                || None,
            ),
            WindowsPasteOutcome::NotPaste
        );
        let (_, exp) = ed.take_and_record();
        submissions.push(exp.trim().to_string());
        submissions
    }

    #[test]
    fn conpty_crlf_with_ctrl_enter_and_shift_modifiers_submits_once() {
        let text = " $ Get-Command litemiyu*\r\n\r\n第二段\t缩进\r\n第三段\r\n";
        assert_eq!(
            replay_conpty_into_editor(text, KeyCode::Enter, true),
            vec![text.replace("\r\n", "\n").trim().to_string()]
        );
        assert_eq!(
            replay_conpty_into_editor(text, KeyCode::Char('j'), true),
            vec![text.replace("\r\n", "\n").trim().to_string()]
        );
    }

    #[test]
    fn conpty_crlf_without_clipboard_never_splits_messages() {
        let text = "first\r\n\r\nsecond\r\nthird";
        for lf in [KeyCode::Enter, KeyCode::Char('j')] {
            assert_eq!(
                replay_conpty_into_editor(text, lf, false),
                vec![text.replace("\r\n", "\n")]
            );
        }
    }

    #[test]
    fn conpty_lf_only_preserves_blank_lines_and_submits_once() {
        let text = "first\n\nsecond\nthird";
        for lf in [KeyCode::Enter, KeyCode::Char('j')] {
            assert_eq!(
                replay_conpty_into_editor(text, lf, true),
                vec![text.to_string()]
            );
        }
    }

    #[test]
    fn normal_typing_followed_by_enter_still_submits() {
        let mut st = WindowsPasteState::default();
        let t0 = Instant::now();
        let mut buf = String::new();
        for (i, ch) in "hello".chars().enumerate() {
            let _ = st.handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &buf,
                false,
                t0 + Duration::from_millis(i as u64 * 90),
                || Some("hello\nworld".into()),
            );
            buf.push(ch);
        }
        let res = st.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &buf,
            false,
            t0 + Duration::from_millis(700),
            || Some("hello\nworld".into()),
        );
        assert_eq!(res, WindowsPasteOutcome::NotPaste);
    }
}
