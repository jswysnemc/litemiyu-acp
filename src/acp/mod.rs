//! ACP v1 JSON-RPC 2.0 异步客户端（stdio 子进程传输）。

pub mod types;

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub use types::*;

enum PendingKind {
    Initialize,
    NewSession,
    LoadSession { session_id: String },
    ListSessions,
    Prompt,
    SetConfigOption,
    SetMode,
}

pub struct AcpClient {
    cmd_tx: mpsc::UnboundedSender<AcpCommand>,
}

impl AcpClient {
    pub fn spawn(
        program: String,
        args: Vec<String>,
        cwd: String,
        event_tx: mpsc::UnboundedSender<AcpEvent>,
    ) -> Result<Self> {
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Unix 下通过 setsid 让 ACP 后端及其派生的后台命令脱离控制终端 /dev/tty，
        // 防止后台子进程访问或争抢前端 TUI 的键盘与鼠标输入流。
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("无法启动 ACP 后端进程：{program} {}", args.join(" ")))?;

        let stdin = child.stdin.take().context("无法获取 ACP stdin")?;
        let stdout = child.stdout.take().context("无法获取 ACP stdout")?;
        let stderr = child.stderr.take().context("无法获取 ACP stderr")?;

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<AcpCommand>();

        // 收集 stderr 最后几行用于诊断
        let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let stderr_clone = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut guard = stderr_clone.lock().await;
                if guard.len() >= 20 {
                    guard.remove(0);
                }
                guard.push(line);
            }
        });

        tokio::spawn(async move {
            let mut writer = stdin;
            let mut reader = BufReader::new(stdout).lines();
            let mut next_id: u64 = 1;
            let mut pending: HashMap<u64, PendingKind> = HashMap::new();

            // 立即发送 initialize 握手
            let init_id = next_id;
            next_id += 1;
            pending.insert(init_id, PendingKind::Initialize);
            let init_req = json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": {
                            "readTextFile": true,
                            "writeTextFile": true
                        },
                        "terminal": false
                    },
                    "clientInfo": {
                        "name": "litemiyu-acp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            });
            if send_json(&mut writer, &init_req).await.is_err() {
                let _ = event_tx.send(AcpEvent::Disconnected("ACP 管道写入失败".into()));
                return;
            }

            loop {
                tokio::select! {
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break; };
                        match cmd {
                            AcpCommand::Shutdown => {
                                let _ = child.kill().await;
                                break;
                            }
                            AcpCommand::NewSession { cwd } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::NewSession);
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/new",
                                    "params": {
                                        "cwd": cwd,
                                        "mcpServers": []
                                    }
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::LoadSession { session_id, cwd } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::LoadSession { session_id: session_id.clone() });
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/load",
                                    "params": {
                                        "sessionId": session_id,
                                        "cwd": cwd,
                                        "mcpServers": []
                                    }
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::ListSessions { cwd } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::ListSessions);
                                let params = match cwd {
                                    Some(c) => json!({ "cwd": c }),
                                    None => json!({}),
                                };
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/list",
                                    "params": params
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::Prompt { session_id, text, images } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::Prompt);
                                let mut prompt_blocks: Vec<Value> = Vec::with_capacity(images.len() + 1);
                                for img in images {
                                    prompt_blocks.push(json!({
                                        "type": "image",
                                        "mimeType": img.mime_type,
                                        "data": img.base64_data
                                    }));
                                }
                                if !text.trim().is_empty() || prompt_blocks.is_empty() {
                                    prompt_blocks.push(json!({ "type": "text", "text": text }));
                                }
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/prompt",
                                    "params": {
                                        "sessionId": session_id,
                                        "prompt": prompt_blocks
                                    }
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::Cancel { session_id } => {
                                let notif = json!({
                                    "jsonrpc": "2.0",
                                    "method": "session/cancel",
                                    "params": { "sessionId": session_id }
                                });
                                let _ = send_json(&mut writer, &notif).await;
                            }
                            AcpCommand::SetConfigOption { session_id, config_id, value } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::SetConfigOption);
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/set_config_option",
                                    "params": {
                                        "sessionId": session_id,
                                        "configId": config_id,
                                        "value": value
                                    }
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::SetMode { session_id, mode_id } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::SetMode);
                                let req = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": "session/set_mode",
                                    "params": {
                                        "sessionId": session_id,
                                        "modeId": mode_id
                                    }
                                });
                                let _ = send_json(&mut writer, &req).await;
                            }
                            AcpCommand::AnswerPermission { rpc_id, option_id } => {
                                let outcome = match option_id {
                                    Some(opt) => json!({ "outcome": "selected", "optionId": opt }),
                                    None => json!({ "outcome": "cancelled" }),
                                };
                                let resp = json!({
                                    "jsonrpc": "2.0",
                                    "id": rpc_id,
                                    "result": { "outcome": outcome }
                                });
                                let _ = send_json(&mut writer, &resp).await;
                            }
                        }
                    }
                    line_res = reader.next_line() => {
                        match line_res {
                            Ok(Some(line)) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
                                    continue;
                                };
                                handle_incoming(&msg, &mut pending, &mut writer, &event_tx).await;
                            }
                            Ok(None) => {
                                let tail = stderr_buf.lock().await.join(" | ");
                                let reason = if tail.is_empty() {
                                    "ACP 后端进程已退出".to_string()
                                } else {
                                    format!("ACP 后端进程已退出: {tail}")
                                };
                                let _ = event_tx.send(AcpEvent::Disconnected(reason));
                                break;
                            }
                            Err(err) => {
                                let _ = event_tx.send(AcpEvent::Disconnected(format!("读取 ACP 输出失败: {err}")));
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self { cmd_tx })
    }

    pub fn send(&self, cmd: AcpCommand) {
        let _ = self.cmd_tx.send(cmd);
    }
}

async fn send_json(writer: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn handle_incoming(
    msg: &Value,
    pending: &mut HashMap<u64, PendingKind>,
    writer: &mut tokio::process::ChildStdin,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id");

    match (method, id) {
        // 服务端通知
        (Some("session/update"), None) => {
            let params = &msg["params"];
            let sid = params["sessionId"].as_str().unwrap_or("").to_string();
            if let Some(update) = parse_session_update(&params["update"]) {
                let _ = event_tx.send(AcpEvent::Update {
                    session_id: sid,
                    update,
                });
            }
        }
        // 服务端反向请求
        (Some(m), Some(req_id)) => {
            handle_server_request(m, req_id, &msg["params"], writer, event_tx).await;
        }
        // 对我们请求的响应
        (None, Some(resp_id)) => {
            let Some(num_id) = resp_id.as_u64() else {
                return;
            };
            let Some(kind) = pending.remove(&num_id) else {
                return;
            };
            if let Some(err) = msg.get("error") {
                let message = err["message"]
                    .as_str()
                    .unwrap_or("未知 RPC 错误")
                    .to_string();
                let _ = event_tx.send(AcpEvent::Error(message));
                if matches!(kind, PendingKind::Prompt) {
                    let _ = event_tx.send(AcpEvent::PromptFinished {
                        stop_reason: "error".into(),
                        usage: None,
                    });
                }
                return;
            }
            let result = &msg["result"];
            match kind {
                PendingKind::Initialize => {
                    let info = &result["agentInfo"];
                    let agent_name = info["name"]
                        .as_str()
                        .unwrap_or("ACP Agent")
                        .to_string();
                    let agent_version = info["version"].as_str().unwrap_or("").to_string();
                    let _ = event_tx.send(AcpEvent::Initialized {
                        agent_name,
                        agent_version,
                    });
                }
                PendingKind::NewSession => {
                    let sid = result["sessionId"].as_str().unwrap_or("").to_string();
                    emit_session_ready(sid, result, false, event_tx);
                }
                PendingKind::LoadSession { session_id } => {
                    emit_session_ready(session_id, result, true, event_tx);
                }
                PendingKind::ListSessions => {
                    let sessions = result["sessions"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    let session_id = s["sessionId"]
                                        .as_str()
                                        .or_else(|| s["id"].as_str())?
                                        .to_string();
                                    let raw_cwd = s["cwd"]
                                        .as_str()
                                        .or_else(|| s["workspace"].as_str())
                                        .or_else(|| s["directory"].as_str())
                                        .or_else(|| s["workingDirectory"].as_str())
                                        .unwrap_or("");
                                    let cwd = decode_acp_cwd(raw_cwd);
                                    let title = s["title"]
                                        .as_str()
                                        .or_else(|| s["name"].as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let updated_at = extract_timestamp_field(s);
                                    Some(SessionSummary {
                                        session_id,
                                        cwd,
                                        title,
                                        updated_at,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = event_tx.send(AcpEvent::SessionsListed(sessions));
                }
                PendingKind::Prompt => {
                    let stop_reason = result["stopReason"]
                        .as_str()
                        .unwrap_or("end_turn")
                        .to_string();
                    let usage = result.get("usage").map(|u| TurnUsage {
                        input_tokens: u["inputTokens"].as_u64().unwrap_or(0),
                        output_tokens: u["outputTokens"].as_u64().unwrap_or(0),
                        cached_read_tokens: u["cachedReadTokens"].as_u64().unwrap_or(0),
                        thought_tokens: u["thoughtTokens"].as_u64().unwrap_or(0),
                        total_tokens: u["totalTokens"].as_u64().unwrap_or(0),
                    });
                    let _ = event_tx.send(AcpEvent::PromptFinished { stop_reason, usage });
                }
                PendingKind::SetConfigOption => {
                    let opts = parse_config_options(&result["configOptions"]);
                    let _ = event_tx.send(AcpEvent::ConfigUpdated(opts));
                }
                PendingKind::SetMode => {}
            }
        }
        _ => {}
    }
}

fn emit_session_ready(
    session_id: String,
    result: &Value,
    is_load: bool,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
) {
    let config_options = parse_config_options(&result["configOptions"]);
    let modes = result["modes"]["availableModes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(ModeInfo {
                        id: m["id"].as_str()?.to_string(),
                        name: m["name"].as_str().unwrap_or("").to_string(),
                        description: m["description"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let current_mode = result["modes"]["currentModeId"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            config_options
                .iter()
                .find(|c| c.id == "mode" || c.category == "mode")
                .map(|c| c.current_value.clone())
        });
    let current_model_name = result["models"]["currentModelId"]
        .as_str()
        .and_then(|cid| {
            result["models"]["availableModels"]
                .as_array()?
                .iter()
                .find(|m| m["modelId"].as_str() == Some(cid))
                .and_then(|m| m["name"].as_str())
                .map(String::from)
        });
    let tier = result["_meta"]["antigravity"]["tier"]["name"]
        .as_str()
        .map(String::from);

    let _ = event_tx.send(AcpEvent::SessionReady {
        session_id,
        config_options,
        modes,
        current_mode,
        current_model_name,
        tier,
        is_load,
    });
}

async fn handle_server_request(
    method: &str,
    req_id: &Value,
    params: &Value,
    writer: &mut tokio::process::ChildStdin,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
) {
    match method {
        "session/request_permission" => {
            let session_id = params["sessionId"].as_str().unwrap_or("").to_string();
            let tc = &params["toolCall"];
            let tool_call_id = tc["toolCallId"].as_str().unwrap_or("").to_string();
            let title = tc["title"]
                .as_str()
                .or_else(|| tc["name"].as_str())
                .unwrap_or("工具权限确认")
                .to_string();
            let kind = tc["kind"].as_str().unwrap_or("other").to_string();
            let raw_input = tc.get("rawInput").cloned();
            let options = params["options"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|o| {
                            Some(PermissionOption {
                                option_id: o["optionId"].as_str()?.to_string(),
                                name: o["name"].as_str().unwrap_or("").to_string(),
                                kind: o["kind"].as_str().unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = event_tx.send(AcpEvent::PermissionRequested(PermissionRequest {
                rpc_id: req_id.clone(),
                session_id,
                tool_call_id,
                title,
                kind,
                raw_input,
                options,
            }));
        }
        "fs/read_text_file" => {
            let path = params["path"].as_str().unwrap_or("");
            let resp = match tokio::fs::read_to_string(path).await {
                Ok(content) => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": { "content": content }
                }),
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": { "code": -32000, "message": e.to_string() }
                }),
            };
            let _ = send_json(writer, &resp).await;
        }
        "fs/write_text_file" => {
            let path = params["path"].as_str().unwrap_or("");
            let content = params["content"].as_str().unwrap_or("");
            let resp = match tokio::fs::write(path, content).await {
                Ok(()) => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {}
                }),
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": { "code": -32000, "message": e.to_string() }
                }),
            };
            let _ = send_json(writer, &resp).await;
        }
        _ => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": { "code": -32601, "message": format!("Unsupported client method: {method}") }
            });
            let _ = send_json(writer, &resp).await;
        }
    }
}

fn parse_config_options(val: &Value) -> Vec<ConfigOption> {
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

fn parse_session_update(u: &Value) -> Option<SessionUpdate> {
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
            } else if u["kind"].as_str() == Some("edit") {
                if let Some(content) = inp["Content"].as_str().or_else(|| inp["content"].as_str()) {
                    out.push(ToolDiff {
                        path,
                        old_text: String::new(),
                        new_text: content.to_string(),
                    });
                }
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

fn extract_timestamp_field(s: &Value) -> String {
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
            } else if let Some(num) = v.as_u64() {
                if num > 0 {
                    return num.to_string();
                }
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
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
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
    use super::decode_acp_cwd;

    #[test]
    fn decodes_percent_encoded_windows_acp_cwd() {
        let raw = "/D:/work/11%E6%9C%88/11.23-DEPLOYC-18352-%E6%97%A0%E9%99%90";
        let got = decode_acp_cwd(raw);
        #[cfg(windows)]
        assert_eq!(got, r"D:\work\11月\11.23-DEPLOYC-18352-无限");
        #[cfg(not(windows))]
        assert_eq!(got, "D:/work/11月/11.23-DEPLOYC-18352-无限");
    }
}

