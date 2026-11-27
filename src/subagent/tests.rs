//! 子代理协议状态、日志增量游标与会话隔离测试。

use super::*;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Logs(PathBuf);

impl Logs {
    /// 创建当前测试专用的空日志根目录并返回清理守卫。
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "agy-tui-agents-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    /// 向 `id` 对应的模拟日志追加原始字节 `text`。
    fn append(&self, id: &str, text: &str) {
        let file = crate::paths::agy_transcript_path(&self.0, id).unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
}

impl Drop for Logs {
    /// 移除本测试创建的临时目录。
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 按 `entry`、`args`、`output` 和 `status` 构造一个 ACP 启动快照。
fn tool<'a>(entry: u64, args: &'a Value, output: &'a str, status: &'a str) -> ToolSnapshot<'a> {
    ToolSnapshot {
        entry_id: entry,
        name: "invoke_subagent",
        title: "invoke_subagent",
        status,
        raw_input: Some(args),
        output,
    }
}

/// 模拟下一次节拍，读取当前 `root` 对应的父子日志。
fn refresh(tracker: &mut Tracker, root: &Path) {
    tracker.enable_local(root.to_path_buf());
    tracker.last_poll = None;
    tracker.poll(Some("parent"));
}

/// 验证启动完成与子任务完成不同，且增量读取不会重复统计或提前处理半行。
#[test]
fn launcher_and_child_have_independent_lifetimes() {
    let logs = Logs::new();
    let mut tracker = Tracker::default();
    let args = json!({"Role":"校对","Model":"预设模型"});
    tracker.update(tool(1, &args, "", "in_progress"));
    tracker.update(tool(
        1,
        &args,
        "Created:\n{\"conversationId\":\"child\"}\nend",
        "completed",
    ));
    assert_eq!(tracker.agents.len(), 1);
    assert_eq!(tracker.agents[0].phase, Phase::Launched);
    assert!(!tracker.watching());
    logs.append("child", "not-json\n{\"type\":\"PLANNER_RESPONSE\",\"model\":\"实际模型\",\"output_tokens\":7,\"status\":\"DONE\",\"tool_calls\":[{\"name\":\"read\"}]}\n{\"type\":");
    refresh(&mut tracker, &logs.0);
    assert_eq!(tracker.agents[0].phase, Phase::Running);
    assert_eq!(tracker.agents[0].current_tool, "read");
    assert_eq!(tracker.agents[0].observed_model, "实际模型");
    refresh(&mut tracker, &logs.0);
    assert_eq!(tracker.agents[0].tokens, 7);
    logs.append("child", "\"PLANNER_RESPONSE\",\"tool_calls\":[{\"name\":\"send_message\",\"args\":{\"Recipient\":\"\\\"parent\\\"\"}}]}\n{\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"完成\",\"output_tokens\":3}\n");
    refresh(&mut tracker, &logs.0);
    assert_eq!(tracker.agents[0].phase, Phase::Completed);
    assert_eq!(tracker.agents[0].tokens, 10);
    assert!(!tracker.watching());
    tracker.update(tool(1, &args, "", "completed"));
    assert_eq!(tracker.agents[0].phase, Phase::Completed);
}

/// 验证缺少 ACP 输出时可由当前父日志关联，且不能复用上次会话的缓存。
#[test]
fn parent_replay_matches_arguments_and_resets() {
    let logs = Logs::new();
    let launch = json!({"type":"PLANNER_RESPONSE", "tool_calls":[{"name":"invoke_subagent","args":{"Role":"\"研究\"","Prompt":"\"任务\""}}]});
    let result = json!({"type":"GENERIC","content":"Created the following subagent:\n{\"conversationId\":\"child\"}"});
    logs.append("parent", &format!("{launch}\n{result}\n"));
    let args = json!({"Role":"研究","Prompt":"任务"});
    let mut tracker = Tracker::default();
    tracker.update(tool(1, &args, "", "completed"));
    refresh(&mut tracker, &logs.0);
    assert_eq!(tracker.agents[0].id.as_deref(), Some("child"));
    tracker.clear();
    tracker.update(tool(2, &args, "", "completed"));
    tracker.poll(Some("unrelated"));
    assert_eq!(tracker.agents[0].id, None);
}

/// 验证多代理去重、失败状态和外部 URI 不会造成任意路径读取。
#[test]
fn group_results_keep_only_valid_distinct_ids() {
    let logs = Logs::new();
    let args = json!({"Role":"检查"});
    let mut tracker = Tracker::default();
    let result = r#"[{"conversationId":"one"},{"conversationId":"two"},{"conversationId":"../bad"},{"conversationId":"one"},{"logAbsoluteUri":"file:///etc/passwd"}]"#;
    tracker.update(tool(1, &args, result, "completed"));
    tracker.update(tool(1, &args, result, "completed"));
    assert_eq!(tracker.agents.len(), 2);
    logs.append("two", "{\"type\":\"SESSION_FAILED\"}\n");
    refresh(&mut tracker, &logs.0);
    assert_eq!(tracker.agents[0].phase, Phase::Launched);
    assert_eq!(tracker.agents[1].phase, Phase::Failed);
}
