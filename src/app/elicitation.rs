//! ACP `elicitation/create`（含 `agy ask_question`）交互式提问表单状态机与按键处理。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value, json};

use crate::acp::{AcpCommand, ElicitationFieldKind, ElicitationRequest};
use crate::clipboard;

use super::App;

/// 单个提问字段的交互编辑状态。
#[derive(Debug, Clone)]
pub struct ElicitationFieldState {
    pub selected_idx: Option<usize>,
    pub checked: Vec<bool>,
    pub text: String,
    pub bool_val: bool,
    pub cursor_opt: usize,
    pub editing_text: bool,
}

/// 活跃中的 `elicitation/create` 表单会话状态。
#[derive(Debug, Clone)]
pub struct ElicitationState {
    pub req: ElicitationRequest,
    pub answers: Vec<ElicitationFieldState>,
    pub active_field: usize,
}

impl ElicitationState {
    /// 根据收到的 `ElicitationRequest` 初始化各字段默认选择与输入焦点。
    ///
    /// 参数:
    /// - `req`: 服务端发起的提问/表单请求
    ///
    /// 返回:
    /// - 初始化后的 `ElicitationState`
    pub fn new(req: ElicitationRequest) -> Self {
        let answers = req
            .fields
            .iter()
            .map(|f| match &f.kind {
                ElicitationFieldKind::SingleSelect(opts) => ElicitationFieldState {
                    selected_idx: None,
                    checked: Vec::new(),
                    text: String::new(),
                    bool_val: true,
                    cursor_opt: 0,
                    editing_text: opts.is_empty(),
                },
                ElicitationFieldKind::MultiSelect { options, .. } => ElicitationFieldState {
                    selected_idx: None,
                    checked: vec![false; options.len()],
                    text: String::new(),
                    bool_val: true,
                    cursor_opt: 0,
                    editing_text: options.is_empty(),
                },
                ElicitationFieldKind::Text { default }
                | ElicitationFieldKind::Number { default, .. } => ElicitationFieldState {
                    selected_idx: None,
                    checked: Vec::new(),
                    text: default.clone(),
                    bool_val: true,
                    cursor_opt: 0,
                    editing_text: true,
                },
                ElicitationFieldKind::Boolean { default } => ElicitationFieldState {
                    selected_idx: None,
                    checked: Vec::new(),
                    text: String::new(),
                    bool_val: *default,
                    cursor_opt: usize::from(!*default),
                    editing_text: false,
                },
            })
            .collect();
        Self {
            req,
            answers,
            active_field: 0,
        }
    }

    /// 判断当前字段是否已完成有效作答（用于多题页签展示 `✓` 标记）。
    pub fn is_field_answered(&self, idx: usize) -> bool {
        let Some(field) = self.req.fields.get(idx) else {
            return false;
        };
        let Some(ans) = self.answers.get(idx) else {
            return false;
        };
        match &field.kind {
            ElicitationFieldKind::SingleSelect(_) => {
                ans.selected_idx.is_some()
                    || (field.other_key.is_some() && !ans.text.trim().is_empty())
            }
            ElicitationFieldKind::MultiSelect { .. } => {
                ans.checked.iter().any(|&c| c)
                    || (field.other_key.is_some() && !ans.text.trim().is_empty())
            }
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. } => {
                !ans.text.trim().is_empty()
            }
            ElicitationFieldKind::Boolean { .. } => true,
        }
    }

    /// 组装符合 `requestedSchema` 校验规则的 `content` JSON 对象；若存在未满足的必填或数值校验则返回错误提示。
    pub fn build_content(&self) -> Result<Value, String> {
        let mut map = Map::new();
        for (idx, (field, ans)) in self.req.fields.iter().zip(self.answers.iter()).enumerate() {
            match &field.kind {
                ElicitationFieldKind::SingleSelect(opts) => {
                    let custom = ans.text.trim();
                    if let Some(other_key) = &field.other_key
                        && !custom.is_empty()
                    {
                        map.insert(other_key.clone(), Value::String(custom.to_string()));
                    }
                    let chosen_val = if let Some(sel) = ans.selected_idx {
                        opts.get(sel).map(|o| o.value.clone())
                    } else if !custom.is_empty() && field.other_key.is_some() {
                        opts.iter()
                            .find(|o| o.value.eq_ignore_ascii_case("other"))
                            .map(|o| o.value.clone())
                    } else {
                        None
                    };
                    if let Some(val) = chosen_val {
                        map.insert(field.key.clone(), Value::String(val));
                    } else if field.required {
                        return Err(format!("第 {} 题尚未选择答案", idx + 1));
                    }
                }
                ElicitationFieldKind::MultiSelect {
                    options,
                    min_items,
                    max_items,
                } => {
                    let custom = ans.text.trim();
                    let mut picked: Vec<Value> = options
                        .iter()
                        .zip(ans.checked.iter())
                        .filter(|&(_, &on)| on)
                        .map(|(opt, _)| Value::String(opt.value.clone()))
                        .collect();
                    if let Some(other_key) = &field.other_key
                        && !custom.is_empty()
                    {
                        map.insert(other_key.clone(), Value::String(custom.to_string()));
                        if let Some(other_opt) =
                            options.iter().find(|o| o.value.eq_ignore_ascii_case("other"))
                            && !picked
                                .iter()
                                .any(|v| v.as_str() == Some(other_opt.value.as_str()))
                        {
                            picked.push(Value::String(other_opt.value.clone()));
                        }
                    }
                    let min_req = min_items.unwrap_or(usize::from(field.required));
                    if picked.len() < min_req && (custom.is_empty() || field.other_key.is_none()) {
                        return Err(format!("第 {} 题请至少勾选 {min_req} 项", idx + 1));
                    }
                    if let Some(max_req) = max_items
                        && picked.len() > *max_req
                    {
                        return Err(format!("第 {} 题最多选择 {max_req} 项", idx + 1));
                    }
                    if !picked.is_empty() || field.required {
                        map.insert(field.key.clone(), Value::Array(picked));
                    }
                }
                ElicitationFieldKind::Text { .. } => {
                    let v = ans.text.trim();
                    if v.is_empty() {
                        if field.required {
                            return Err(format!("第 {} 题不能为空", idx + 1));
                        }
                    } else {
                        map.insert(field.key.clone(), Value::String(v.to_string()));
                    }
                }
                ElicitationFieldKind::Number { integer, .. } => {
                    let v = ans.text.trim();
                    if v.is_empty() {
                        if field.required {
                            return Err(format!("第 {} 题请输入数字", idx + 1));
                        }
                    } else if *integer {
                        let parsed = v
                            .parse::<i64>()
                            .map_err(|_| format!("第 {} 题需要填写整数", idx + 1))?;
                        map.insert(field.key.clone(), json!(parsed));
                    } else {
                        let parsed = v
                            .parse::<f64>()
                            .ok()
                            .filter(|n| n.is_finite())
                            .ok_or_else(|| format!("第 {} 题需要填写有效数字", idx + 1))?;
                        map.insert(field.key.clone(), json!(parsed));
                    }
                }
                ElicitationFieldKind::Boolean { .. } => {
                    map.insert(field.key.clone(), Value::Bool(ans.bool_val));
                }
            }
        }
        Ok(Value::Object(map))
    }
}

impl App {
    /// 处理活跃中 `elicitation/create` 提问抽屉的键盘交互。
    ///
    /// 参数:
    /// - `key`: crossterm 键盘事件
    pub(super) fn handle_elicitation_key(&mut self, key: KeyEvent) {
        let Some(state) = self.elicitation.as_mut() else {
            return;
        };

        // 1. Ctrl+C 随时取消本次提问并返回 cancelled
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            self.answer_elicitation("cancel", None);
            return;
        }

        // 2. URL 模式或无表单字段时，支持打开链接、确认继续或取消
        if state.req.mode == "url" || state.req.fields.is_empty() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.answer_elicitation("accept", Some(json!({})));
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    if let Some(url) = &state.req.url {
                        clipboard::open_target(url);
                        self.toast("已在浏览器打开确认链接", false);
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.answer_elicitation("cancel", None);
                }
                _ => {}
            }
            return;
        }

        let field_count = state.req.fields.len();
        let active = state.active_field.min(field_count - 1);
        state.active_field = active;

        // 3. 多题左右/Tab 切换（纯文本编辑态保留 Left/Right，仅 Tab/BackTab 切题）
        let is_pure_text = matches!(
            state.req.fields[active].kind,
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. }
        );
        if field_count > 1 {
            let switch_prev = key.code == KeyCode::BackTab
                || (!state.answers[active].editing_text && key.code == KeyCode::Left);
            let switch_next = key.code == KeyCode::Tab
                || (!state.answers[active].editing_text && key.code == KeyCode::Right);
            if switch_prev {
                state.active_field = if active == 0 {
                    field_count - 1
                } else {
                    active - 1
                };
                return;
            }
            if switch_next {
                state.active_field = (active + 1) % field_count;
                return;
            }
        }

        // 4. Ctrl+S / Ctrl+Enter 直接校验并提交整份表单
        if (key.code == KeyCode::Char('s') && key.modifiers == KeyModifiers::CONTROL)
            || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.try_submit_elicitation();
            return;
        }

        let has_other = state.req.fields[active].other_key.is_some();
        let opt_len = match &state.req.fields[active].kind {
            ElicitationFieldKind::SingleSelect(opts)
            | ElicitationFieldKind::MultiSelect { options: opts, .. } => opts.len(),
            ElicitationFieldKind::Boolean { .. } => 2,
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Number { .. } => 0,
        };
        let row_count = opt_len + usize::from(has_other);

        // 5. 文本输入态（纯文本框或 Write-in 自定义输入）
        if state.answers[active].editing_text {
            match key.code {
                KeyCode::Esc => {
                    if is_pure_text {
                        self.answer_elicitation("cancel", None);
                    } else {
                        state.answers[active].editing_text = false;
                    }
                }
                KeyCode::Backspace => {
                    state.answers[active].text.pop();
                }
                KeyCode::Up if !is_pure_text && opt_len > 0 => {
                    state.answers[active].editing_text = false;
                    state.answers[active].cursor_opt = opt_len - 1;
                }
                KeyCode::Down if !is_pure_text && opt_len > 0 => {
                    state.answers[active].editing_text = false;
                    state.answers[active].cursor_opt = 0;
                }
                KeyCode::Enter => {
                    if let ElicitationFieldKind::SingleSelect(opts) = &state.req.fields[active].kind
                        && !state.answers[active].text.trim().is_empty()
                        && let Some(other_idx) =
                            opts.iter().position(|o| o.value.eq_ignore_ascii_case("other"))
                    {
                        state.answers[active].selected_idx = Some(other_idx);
                    }
                    if active + 1 < field_count {
                        state.answers[active].editing_text = false;
                        state.active_field = active + 1;
                    } else {
                        self.try_submit_elicitation();
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    state.answers[active].text.push(c);
                }
                _ => {}
            }
            return;
        }

        // 6. 选项导航态（单选 / 多选 / 布尔）
        match key.code {
            KeyCode::Esc => {
                self.answer_elicitation("cancel", None);
            }
            KeyCode::Up if row_count > 0 => {
                let cur = state.answers[active].cursor_opt;
                state.answers[active].cursor_opt = if cur == 0 { row_count - 1 } else { cur - 1 };
            }
            KeyCode::Down if row_count > 0 => {
                let cur = state.answers[active].cursor_opt;
                state.answers[active].cursor_opt = (cur + 1) % row_count;
            }
            KeyCode::Char('i') | KeyCode::Char('w') if has_other => {
                state.answers[active].cursor_opt = opt_len;
                state.answers[active].editing_text = true;
            }
            KeyCode::Char(' ') => {
                let cur = state.answers[active].cursor_opt;
                if has_other && cur == opt_len {
                    state.answers[active].editing_text = true;
                    return;
                }
                match &state.req.fields[active].kind {
                    ElicitationFieldKind::SingleSelect(opts) => {
                        if cur < opts.len() {
                            state.answers[active].selected_idx = Some(cur);
                            if has_other && opts[cur].value.eq_ignore_ascii_case("other") {
                                state.answers[active].editing_text = true;
                            }
                        }
                    }
                    ElicitationFieldKind::MultiSelect { .. } => {
                        if let Some(cell) = state.answers[active].checked.get_mut(cur) {
                            *cell = !*cell;
                        }
                    }
                    ElicitationFieldKind::Boolean { .. } => {
                        state.answers[active].bool_val = cur == 0;
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                let idx = (c as usize) - ('1' as usize);
                if idx >= opt_len {
                    return;
                }
                state.answers[active].cursor_opt = idx;
                match &state.req.fields[active].kind {
                    ElicitationFieldKind::SingleSelect(opts) => {
                        state.answers[active].selected_idx = Some(idx);
                        if has_other
                            && opts[idx].value.eq_ignore_ascii_case("other")
                            && state.answers[active].text.trim().is_empty()
                        {
                            state.answers[active].editing_text = true;
                        } else if active + 1 < field_count {
                            state.active_field = active + 1;
                        } else {
                            self.try_submit_elicitation();
                        }
                    }
                    ElicitationFieldKind::MultiSelect { .. } => {
                        if let Some(cell) = state.answers[active].checked.get_mut(idx) {
                            *cell = !*cell;
                        }
                    }
                    ElicitationFieldKind::Boolean { .. } => {
                        state.answers[active].bool_val = idx == 0;
                        if active + 1 < field_count {
                            state.active_field = active + 1;
                        } else {
                            self.try_submit_elicitation();
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Enter => {
                let cur = state.answers[active].cursor_opt;
                if has_other && cur == opt_len {
                    if state.answers[active].text.trim().is_empty() {
                        state.answers[active].editing_text = true;
                    } else if active + 1 < field_count {
                        state.active_field = active + 1;
                    } else {
                        self.try_submit_elicitation();
                    }
                    return;
                }
                match &state.req.fields[active].kind {
                    ElicitationFieldKind::SingleSelect(opts) => {
                        if cur < opts.len() {
                            state.answers[active].selected_idx = Some(cur);
                            if has_other
                                && opts[cur].value.eq_ignore_ascii_case("other")
                                && state.answers[active].text.trim().is_empty()
                            {
                                state.answers[active].editing_text = true;
                                return;
                            }
                        }
                        if active + 1 < field_count {
                            state.active_field = active + 1;
                        } else {
                            self.try_submit_elicitation();
                        }
                    }
                    ElicitationFieldKind::MultiSelect { min_items, .. } => {
                        let any_checked = state.answers[active].checked.iter().any(|&c| c);
                        let min_req = min_items
                            .unwrap_or(usize::from(state.req.fields[active].required));
                        if !any_checked
                            && min_req > 0
                            && state.answers[active].text.trim().is_empty()
                            && cur < opt_len
                        {
                            // 若用户尚未按空格勾选且当前为必填题，Enter 直接选中当前光标项
                            state.answers[active].checked[cur] = true;
                        }
                        if active + 1 < field_count {
                            state.active_field = active + 1;
                        } else {
                            self.try_submit_elicitation();
                        }
                    }
                    ElicitationFieldKind::Boolean { .. } => {
                        state.answers[active].bool_val = cur == 0;
                        if active + 1 < field_count {
                            state.active_field = active + 1;
                        } else {
                            self.try_submit_elicitation();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn try_submit_elicitation(&mut self) {
        let Some(state) = &self.elicitation else {
            return;
        };
        match state.build_content() {
            Ok(content) => {
                self.answer_elicitation("accept", Some(content));
            }
            Err(err) => {
                self.toast(err, true);
            }
        }
    }

    fn answer_elicitation(&mut self, action: &str, content: Option<Value>) {
        if let Some(state) = self.elicitation.take()
            && let Some(client) = &self.client
        {
            client.send(AcpCommand::AnswerElicitation {
                rpc_id: state.req.rpc_id,
                action: action.to_string(),
                content,
            });
        }
    }
}
