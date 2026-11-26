//! 从 ACP 启动工具的入参与返回值提取展示字段，不采用后端指定的任意文件路径。

use serde_json::Value;

use crate::paths::{is_safe_session_id, parse_file_uri};

#[derive(Clone, Debug, Default)]
pub(super) struct Launch {
    pub role: String,
    pub model: String,
    pub kind: String,
    pub task: String,
}

/// 读取 `args` 中首个匹配 `keys` 的文本，兼容日志双重编码的字符串。
pub(super) fn field(args: &Value, keys: &[&str]) -> String {
    let raw = keys
        .iter()
        .find_map(|key| args[*key].as_str())
        .unwrap_or("");
    let decoded = serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_owned());
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Launch {
    /// 将 `args` 转换为角色、模型、类型和任务摘要，缺失的字段保持为空。
    pub fn from_args(args: &Value) -> Self {
        Self {
            role: field(args, &["Role", "role", "Name", "name"]),
            model: field(args, &["Model", "model"]),
            kind: field(args, &["TypeName", "typeName", "type_name"]),
            task: field(args, &["Prompt", "prompt", "Task", "task"])
                .chars()
                .take(300)
                .collect(),
        }
    }

    /// 根据 `other` 的非空字段确认两个来源描述了相同的启动请求。
    pub fn matches(&self, other: &Self) -> bool {
        (!self.role.is_empty() || !self.task.is_empty())
            && self.role == other.role
            && self.task == other.task
    }
}

/// 【子代理】【解析标识】从结构化结果或带说明的 JSON 中提取去重的合法会话标识。
pub(super) fn child_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    // 1. 逐个解析完整 JSON 值，同时兼容说明文字包裹的多代理结果
    while let Some(start) = rest.find(['{', '[']) {
        rest = &rest[start..];
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            collect_ids(&value, &mut ids);
            rest = &rest[stream.byte_offset()..];
        } else {
            rest = &rest[1..];
        }
    }
    ids
}

/// 收集 `value` 中明确命名的子会话标识；日志 URI 仅用于识别标识，不用作读取路径。
fn collect_ids(value: &Value, ids: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            let explicit = field(value, &["conversationId", "conversation_id"]);
            let uri_id = || {
                let path = parse_file_uri(value.get("logAbsoluteUri")?.as_str()?)?;
                if !path.ends_with(".system_generated/logs/transcript.jsonl") {
                    return None;
                }
                let log_dir = path.ancestors().nth(3)?;
                Some(log_dir.file_name()?.to_str()?.to_owned())
            };
            let candidate = if explicit.is_empty() {
                uri_id()
            } else {
                Some(explicit)
            };
            if let Some(id) = candidate
                && is_safe_session_id(&id)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
            for child in object.values() {
                collect_ids(child, ids);
            }
        }
        Value::Array(children) => {
            for child in children {
                collect_ids(child, ids);
            }
        }
        _ => {}
    }
}
