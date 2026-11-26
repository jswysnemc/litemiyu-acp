//! ACP 回放若缺少启动返回值，仅从当前父会话关联具有明确匹配信息的子会话。

use std::path::Path;

use super::{Agent, Cursor, Launch, child_ids, is_subagent_family_tool};

#[derive(Default)]
pub(super) struct ParentLog {
    cursor: Cursor,
    pending: Vec<(String, Launch)>,
    launches: Vec<(String, Launch, String)>,
}

impl ParentLog {
    /// 【子代理】【关联父日志】在 `root` 读取 `parent` 的新记录，为匹配的 `agents` 补齐标识。
    pub fn reconcile(&mut self, root: &Path, parent: &str, agents: &mut [Agent]) -> bool {
        for row in self.cursor.read(root, parent) {
            match row["type"].as_str() {
                Some("PLANNER_RESPONSE") => {
                    self.pending.clear();
                    if let Some([call]) = row["tool_calls"].as_array().map(Vec::as_slice) {
                        let name = call["name"].as_str().unwrap_or("");
                        if is_subagent_family_tool(name) {
                            self.pending
                                .push((name.into(), Launch::from_args(&call["args"])));
                        }
                    }
                }
                Some("GENERIC") if self.pending.len() == 1 => {
                    let content = row["content"].as_str().unwrap_or("");
                    if let Some((_, result)) = content.split_once("Created the following subagent:")
                    {
                        let (name, launch) = &self.pending[0];
                        for id in child_ids(result) {
                            if !self.launches.iter().any(|(_, _, seen)| seen == &id) {
                                self.launches.push((name.clone(), launch.clone(), id));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut changed = false;
        for index in 0..agents.len() {
            let agent = &agents[index];
            if agent.id.is_some() {
                continue;
            }
            let mut matches = self.launches.iter().filter(|(name, launch, id)| {
                name == &agent.tool
                    && launch.matches(&agent.launch)
                    && !agents.iter().any(|other| other.id.as_ref() == Some(id))
            });
            let Some((_, _, id)) = matches.next() else {
                continue;
            };
            if matches.next().is_none()
                && agents
                    .iter()
                    .filter(|other| {
                        other.id.is_none()
                            && other.tool == agent.tool
                            && other.launch.matches(&agent.launch)
                    })
                    .count()
                    == 1
            {
                agents[index].id = Some(id.clone());
                changed = true;
            }
        }
        changed
    }
}
