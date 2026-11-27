//! ACP v1 JSON-RPC 2.0 异步客户端（stdio 子进程传输）。

mod parse;
pub mod types;

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub use parse::decode_acp_cwd;
pub use types::*;
use parse::{
    extract_timestamp_field, parse_config_options, parse_elicitation_request, parse_session_update,
};

enum PendingKind {
    Initialize,
    NewSession,
    LoadSession { session_id: String },
    ListSessions,
    Prompt { prompt_id: u64 },
    SetConfigOption,
    SetMode,
}

pub struct AcpClient {
    cmd_tx: mpsc::UnboundedSender<AcpCommand>,
    #[cfg_attr(not(unix), allow(dead_code))]
    child_pid: Option<u32>,
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
        let child_pid = child.id();

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
                        "terminal": false,
                        "elicitation": {
                            "form": {}
                        }
                    },
                    "clientInfo": {
                        "name": "agy-tui",
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
                                terminate_process_group(child_pid);
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
                            AcpCommand::Prompt { prompt_id, session_id, text, images } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingKind::Prompt { prompt_id });
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
                            AcpCommand::AnswerElicitation { rpc_id, action, content } => {
                                let mut result = serde_json::Map::new();
                                result.insert("action".into(), Value::String(action));
                                if let Some(c) = content {
                                    result.insert("content".into(), c);
                                }
                                let resp = json!({
                                    "jsonrpc": "2.0",
                                    "id": rpc_id,
                                    "result": Value::Object(result)
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

        Ok(Self { cmd_tx, child_pid })
    }

    pub fn send(&self, cmd: AcpCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    #[cfg(test)]
    pub fn dummy_for_test(cmd_tx: mpsc::UnboundedSender<AcpCommand>) -> Self {
        Self {
            cmd_tx,
            child_pid: None,
        }
    }

    /// 优雅终止 ACP 后端及其独立进程组，避免退出客户端后残留后台孤儿进程。
    pub async fn shutdown(&self) {
        // 1. 通知异步任务退出并优先向整个 Unix 进程组发送 SIGTERM
        let _ = self.cmd_tx.send(AcpCommand::Shutdown);
        #[cfg(unix)]
        if let Some(pid) = self.child_pid {
            send_signal_to_group(pid, libc::SIGTERM);
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            send_signal_to_group(pid, libc::SIGKILL);
        }
    }
}

/// 终止独立会话进程组（Unix 下 `setsid()` 后的子进程 PID 即为其进程组 ID）。
fn terminate_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(p) = pid {
        send_signal_to_group(p, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(unix)]
fn send_signal_to_group(pid: u32, sig: libc::c_int) {
    if let Ok(pgid) = i32::try_from(pid)
        && pgid > 1
    {
        unsafe {
            let _ = libc::kill(-pgid, sig);
        }
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
                match kind {
                    PendingKind::Prompt { prompt_id } => {
                        let _ = event_tx.send(AcpEvent::PromptRejected { prompt_id, message });
                    }
                    _ => {
                        let _ = event_tx.send(AcpEvent::Error(message));
                    }
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
                PendingKind::Prompt { prompt_id } => {
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
                    let _ = event_tx.send(AcpEvent::PromptFinished {
                        prompt_id,
                        stop_reason,
                        usage,
                    });
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
        "elicitation/create" => {
            let req = parse_elicitation_request(req_id, params);
            let _ = event_tx.send(AcpEvent::ElicitationRequested(req));
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
