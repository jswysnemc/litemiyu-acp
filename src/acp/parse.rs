//! ACP v1 报文解析：配置选项、会话流式更新、工具 Diff/输出提取与跨平台路径解码。

use serde_json::Value;

use super::types::{
    ConfigChoice, ConfigOption, ElicitationField, ElicitationFieldKind, ElicitationOption,
    ElicitationRequest, PlanItem, SessionUpdate, SlashCommandInfo, ToolDiff,
};

pub(super) fn parse_config_options(val: &Value) -> Vec<ConfigOption> {
    val.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c["id"].as_str()?.to_string();
                    let name = c["name"].as_str().unwrap_or(&id).to_string();
                    let category = c["category"].as_str().unwrap_or("").to_string();
                    let current_value = c["currentValue"].as_str().unwrap_or("").to_string();
                    let options = c["options"]
                        .as_array()
                        .map(|opts| {
                            opts.iter()
                                .filter_map(|o| {
                                    Some(ConfigChoice {
                                        value: o["value"].as_str()?.to_string(),
                                        name: o["name"].as_str().unwrap_or("").to_string(),
                                        description: o["description"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(ConfigOption {
                        id,
                        name,
                        category,
                        current_value,
                        options,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_session_update(u: &Value) -> Option<SessionUpdate> {
    let kind = u["sessionUpdate"].as_str()?;
    match kind {
        "user_message_chunk" => Some(SessionUpdate::UserMessageChunk(extract_text(&u["content"]))),
        "agent_message_chunk" => Some(SessionUpdate::AgentMessageChunk(extract_text(&u["content"]))),
        "agent_thought_chunk" => Some(SessionUpdate::AgentThoughtChunk(extract_text(&u["content"]))),
        "tool_call" => Some(SessionUpdate::ToolCall {
            call_id: u["toolCallId"].as_str().unwrap_or("").to_string(),
            name: u["name"].as_str().unwrap_or("tool").to_string(),
            title: clean_tool_title(
                u["title"]
                    .as_str()
                    .or_else(|| u["name"].as_str())
                    .unwrap_or("tool"),
            ),
            kind: u["kind"].as_str().unwrap_or("other").to_string(),
            status: u["status"].as_str().unwrap_or("in_progress").to_string(),
            raw_input: u.get("rawInput").or_else(|| u.get("args")).cloned(),
            output: extract_tool_output(u),
            diffs: extract_tool_diffs(u),
        }),
        "tool_call_update" => Some(SessionUpdate::ToolCallUpdate {
            call_id: u["toolCallId"].as_str().unwrap_or("").to_string(),
            name: u["name"].as_str().map(String::from),
            title: u["title"].as_str().map(clean_tool_title),
            kind: u["kind"].as_str().map(String::from),
            status: u["status"].as_str().map(String::from),
            raw_input: u.get("rawInput").or_else(|| u.get("args")).cloned(),
            output: extract_tool_output(u),
            diffs: extract_tool_diffs(u),
        }),
        "plan" => {
            let entries = u["entries"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            Some(PlanItem {
                                content: e["content"].as_str()?.to_string(),
                                status: e["status"].as_str().unwrap_or("pending").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(SessionUpdate::Plan(entries))
        }
        "available_commands_update" => {
            let cmds = u["availableCommands"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| {
                            Some(SlashCommandInfo {
                                name: c["name"].as_str()?.to_string(),
                                description: c["description"].as_str().unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(SessionUpdate::AvailableCommands(cmds))
        }
        "config_option_update" => {
            Some(SessionUpdate::ConfigOptions(parse_config_options(&u["configOptions"])))
        }
        "current_mode_update" => u["currentModeId"]
            .as_str()
            .map(|m| SessionUpdate::CurrentMode(m.to_string())),
        "session_info_update" => u["title"]
            .as_str()
            .map(|t| SessionUpdate::SessionTitle(t.to_string())),
        "usage_update" => Some(SessionUpdate::Usage {
            used: u["used"].as_u64().unwrap_or(0),
            size: u["size"].as_u64().unwrap_or(0),
        }),
        _ => None,
    }
}

fn extract_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if content["type"].as_str() == Some("image") {
        return "[图片]".to_string();
    }
    if let Some(s) = content["text"].as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .filter_map(|item| {
                if item["type"].as_str() == Some("image") {
                    Some("[图片]")
                } else {
                    item["text"].as_str()
                }
            })
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn clean_tool_title(raw: &str) -> String {
    for prefix in ["File ", "Read ", "Reading "] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            if rest.starts_with('/') && rest.len() >= 3 && rest.as_bytes()[2] == b':' {
                return decode_acp_cwd(rest);
            }
            if prefix == "File " {
                return rest.to_string();
            }
        }
    }
    raw.to_string()
}

fn extract_tool_diffs(u: &Value) -> Vec<ToolDiff> {
    let mut out = Vec::new();
    if let Some(arr) = u["content"].as_array() {
        for item in arr {
            if item["type"].as_str() == Some("diff") {
                let raw_path = item["path"].as_str().unwrap_or("");
                let path = decode_acp_cwd(raw_path);
                let old_text = item["oldText"]
                    .as_str()
                    .or_else(|| item["old_text"].as_str())
                    .unwrap_or("")
                    .to_string();
                let new_text = item["newText"]
                    .as_str()
                    .or_else(|| item["new_text"].as_str())
                    .unwrap_or("")
                    .to_string();
                if !old_text.is_empty() || !new_text.is_empty() {
                    out.push(ToolDiff {
                        path,
                        old_text,
                        new_text,
                    });
                }
            }
        }
    }
    // 如果 content 中尚未提供 diff（如编辑进行中或某些后端仅在 rawInput 给出 Target/Replacement），用 rawInput 兜底
    if out.is_empty() {
        let input = u.get("rawInput").or_else(|| u.get("args"));
        if let Some(inp) = input {
            let path = inp["Path"]
                .as_str()
                .or_else(|| inp["path"].as_str())
                .map(decode_acp_cwd)
                .unwrap_or_default();
            if let (Some(old), Some(new)) = (
                inp["Target"].as_str().or_else(|| inp["old_str"].as_str()),
                inp["Replacement"]
                    .as_str()
                    .or_else(|| inp["new_str"].as_str()),
            ) {
                out.push(ToolDiff {
                    path,
                    old_text: old.to_string(),
                    new_text: new.to_string(),
                });
            } else if u["kind"].as_str() == Some("edit")
                && let Some(content) = inp["Content"].as_str().or_else(|| inp["content"].as_str()) {
                    out.push(ToolDiff {
                        path,
                        old_text: String::new(),
                        new_text: content.to_string(),
                    });
                }
        }
    }
    out
}

fn extract_tool_output(u: &Value) -> Option<String> {
    if let Some(arr) = u["content"].as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if item["type"].as_str() == Some("diff") {
                // diff 已由 extract_tool_diffs 结构化渲染，不再输出占位文本
                continue;
            }
            if let Some(t) = item["content"]["text"].as_str() {
                parts.push(t.to_string());
            } else if let Some(t) = item["text"].as_str() {
                parts.push(t.to_string());
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    if let Some(raw) = u.get("rawOutput") {
        if let Some(s) = raw.as_str() {
            return Some(s.to_string());
        }
        if let Some(s) = raw["output"]
            .as_str()
            .or_else(|| raw["content"].as_str())
            .or_else(|| raw["text"].as_str())
        {
            return Some(s.to_string());
        }
        if raw.as_object().is_some_and(|m| m.is_empty()) {
            return None;
        }
        if !raw.is_null() {
            return Some(serde_json::to_string_pretty(raw).unwrap_or_default());
        }
    }
    None
}

pub(super) fn extract_timestamp_field(s: &Value) -> String {
    for key in [
        "updatedAt",
        "updated_at",
        "lastUpdatedAt",
        "modifiedAt",
        "createdAt",
        "created_at",
        "timestamp",
    ] {
        if let Some(v) = s.get(key) {
            if let Some(str_val) = v.as_str() {
                let trimmed = str_val.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            } else if let Some(num) = v.as_i64() {
                if num > 0 {
                    return num.to_string();
                }
            } else if let Some(num) = v.as_u64()
                && num > 0 {
                    return num.to_string();
                }
        }
    }
    String::new()
}

/// 将 `agy acp` 在 Windows 下返回的 `/D:/work/11%E6%9C%88/...` 解码成干净路径 `D:\work\11月\...`。
pub fn decode_acp_cwd(raw: &str) -> String {
    let decoded = percent_decode(raw);
    let stripped = if decoded.len() >= 3
        && decoded.starts_with('/')
        && decoded.as_bytes()[2] == b':'
    {
        &decoded[1..]
    } else {
        decoded.as_str()
    };
    #[cfg(windows)]
    {
        stripped.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        stripped.to_string()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// 解析服务端发起的 `elicitation/create` 请求及 JSON Schema 字段定义。
///
/// 参数:
/// - `req_id`: JSON-RPC 请求 ID
/// - `params`: `elicitation/create` 参数对象
///
/// 返回:
/// - 规范化后的 `ElicitationRequest`
pub(super) fn parse_elicitation_request(req_id: &Value, params: &Value) -> ElicitationRequest {
    let session_id = params["sessionId"]
        .as_str()
        .or_else(|| params["session_id"].as_str())
        .unwrap_or("")
        .to_string();
    let elicitation_id = params["elicitationId"]
        .as_str()
        .or_else(|| params["elicitation_id"].as_str())
        .unwrap_or("")
        .to_string();
    let url = params["url"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let mode = params["mode"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(if url.is_some() && params.get("requestedSchema").is_none() {
            "url"
        } else {
            "form"
        })
        .to_string();
    let message = params["message"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("代理需要您的回复以继续")
        .to_string();

    let fields = parse_elicitation_fields(&params["requestedSchema"]);
    ElicitationRequest {
        rpc_id: req_id.clone(),
        session_id,
        elicitation_id,
        mode,
        message,
        url,
        fields,
    }
}

/// 将 `requestedSchema.properties` 转为有序字段列表，并自动合并 `agy` 伴随生成的 `q{i}_other` 字段。
fn parse_elicitation_fields(schema: &Value) -> Vec<ElicitationField> {
    let Some(props) = schema["properties"].as_object() else {
        return Vec::new();
    };
    let required_set: std::collections::HashSet<&str> = schema["required"]
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort_by(|a, b| question_key_order(a).cmp(&question_key_order(b)));

    let mut fields: Vec<ElicitationField> = Vec::new();
    let mut consumed_other: std::collections::HashSet<String> = std::collections::HashSet::new();

    for key in &keys {
        if consumed_other.contains(key.as_str()) {
            continue;
        }
        let Some(prop) = props.get(key.as_str()) else {
            continue;
        };
        let required = required_set.contains(key.as_str());
        let mut field = parse_single_elicitation_field(key, prop, required);

        // 1. 若存在伴随自由输入字段 `<key>_other`（如 `q0` + `q0_other`），合入当前选项字段
        if matches!(
            field.kind,
            ElicitationFieldKind::SingleSelect(_) | ElicitationFieldKind::MultiSelect { .. }
        ) {
            let other_name = format!("{key}_other");
            if let Some(other_prop) = props.get(&other_name)
                && other_prop["type"].as_str() == Some("string")
            {
                field.other_title = other_prop["title"]
                    .as_str()
                    .or_else(|| other_prop["description"].as_str())
                    .unwrap_or("自定义输入")
                    .to_string();
                field.other_key = Some(other_name.clone());
                consumed_other.insert(other_name);
            }
        }
        fields.push(field);
    }
    fields
}

fn question_key_order(key: &str) -> (u8, usize, u8, &str) {
    if let Some(rest) = key.strip_prefix('q') {
        if let Some(num_str) = rest.strip_suffix("_other")
            && let Ok(n) = num_str.parse::<usize>()
        {
            return (0, n, 1, key);
        }
        if let Ok(n) = rest.parse::<usize>() {
            return (0, n, 0, key);
        }
    }
    (1, 0, 0, key)
}

fn parse_single_elicitation_field(key: &str, prop: &Value, required: bool) -> ElicitationField {
    let title = prop["title"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(key)
        .to_string();
    let description = prop["description"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let kind_str = prop["type"].as_str().unwrap_or("string");

    let kind = match kind_str {
        "boolean" => ElicitationFieldKind::Boolean {
            default: prop["default"].as_bool().unwrap_or(true),
        },
        "integer" | "number" => ElicitationFieldKind::Number {
            integer: kind_str == "integer",
            default: prop
                .get("default")
                .and_then(|v| {
                    v.as_i64()
                        .map(|n| n.to_string())
                        .or_else(|| v.as_f64().map(|n| n.to_string()))
                        .or_else(|| v.as_str().map(String::from))
                })
                .unwrap_or_default(),
        },
        "array" => {
            let items = &prop["items"];
            let options = extract_schema_options(items);
            let min_items = prop["minItems"].as_u64().map(|v| v as usize);
            let max_items = prop["maxItems"].as_u64().map(|v| v as usize);
            ElicitationFieldKind::MultiSelect {
                options,
                min_items,
                max_items,
            }
        }
        _ => {
            let options = extract_schema_options(prop);
            if options.is_empty() {
                ElicitationFieldKind::Text {
                    default: prop["default"].as_str().unwrap_or("").to_string(),
                }
            } else {
                ElicitationFieldKind::SingleSelect(options)
            }
        }
    };

    ElicitationField {
        key: key.to_string(),
        title,
        description,
        required,
        kind,
        other_key: None,
        other_title: String::new(),
    }
}

/// 从 JSON Schema 属性或 `items` 中提取单选/多选候选项（兼容 `oneOf`、`anyOf` 及 `enum`+`enumNames`）。
fn extract_schema_options(node: &Value) -> Vec<ElicitationOption> {
    if let Some(arr) = node["oneOf"].as_array().or_else(|| node["anyOf"].as_array()) {
        let mut out = Vec::new();
        for item in arr {
            let Some(val) = item["const"].as_str() else {
                continue;
            };
            let label = item["title"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(val)
                .to_string();
            let description = item["description"]
                .as_str()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            out.push(ElicitationOption {
                value: val.to_string(),
                label,
                description,
            });
        }
        if !out.is_empty() {
            return out;
        }
    }

    if let Some(arr) = node["enum"].as_array() {
        let names = node["enumNames"].as_array();
        return arr
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let val = v.as_str()?.to_string();
                let label = names
                    .and_then(|ns| ns.get(i))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&val)
                    .to_string();
                Some(ElicitationOption {
                    value: val,
                    label,
                    description: String::new(),
                })
            })
            .collect();
    }

    Vec::new()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ElicitationFieldKind, decode_acp_cwd, parse_elicitation_request};

    #[test]
    fn decodes_percent_encoded_windows_acp_cwd() {
        let raw = "/D:/work/11%E6%9C%88/11.23-DEPLOYC-18352-%E6%97%A0%E9%99%90";
        let got = decode_acp_cwd(raw);
        #[cfg(windows)]
        assert_eq!(got, r"D:\work\11月\11.23-DEPLOYC-18352-无限");
        #[cfg(not(windows))]
        assert_eq!(got, "D:/work/11月/11.23-DEPLOYC-18352-无限");
    }

    #[test]
    fn parses_agy_question_elicitation_schema_with_other_and_numerical_order() {
        let params = json!({
            "mode": "form",
            "message": "The agent has 2 questions.",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q10": {
                        "type": "array",
                        "title": "Select targets",
                        "items": {
                            "anyOf": [
                                { "const": "linux", "title": "Linux" },
                                { "const": "macos", "title": "macOS" }
                            ]
                        }
                    },
                    "q0": {
                        "type": "string",
                        "title": "Which mode?",
                        "oneOf": [
                            { "const": "fast", "title": "Fast mode" },
                            { "const": "Other", "title": "Other" }
                        ]
                    },
                    "q0_other": {
                        "type": "string",
                        "title": "Optional free-form answer to: Which mode?"
                    }
                },
                "required": ["q0"]
            }
        });
        let req = parse_elicitation_request(&json!(7), &params);
        assert_eq!(req.mode, "form");
        assert_eq!(req.fields.len(), 2);
        assert_eq!(req.fields[0].key, "q0");
        assert!(req.fields[0].required);
        assert_eq!(req.fields[0].other_key.as_deref(), Some("q0_other"));
        assert!(matches!(
            req.fields[0].kind,
            ElicitationFieldKind::SingleSelect(ref opts) if opts.len() == 2
        ));
        assert_eq!(req.fields[1].key, "q10");
        assert!(matches!(
            req.fields[1].kind,
            ElicitationFieldKind::MultiSelect { ref options, .. } if options.len() == 2
        ));
    }
}

