//! 正文折叠分区与展开状态。

use super::Transcript;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldSection {
    Thought,
    ToolCommand,
    ToolPath,
    ToolOutput,
    ToolReadPreview,
    ToolDiff(usize),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FoldReveal {
    extra_lines: usize,
    show_all: bool,
}

impl FoldReveal {
    /// 给定默认可见行数 `base`，返回当前分区的视觉行上限。
    pub fn effective_limit(self, base: usize) -> usize {
        if self.show_all {
            usize::MAX
        } else {
            base.saturating_add(self.extra_lines)
        }
    }
}

impl Transcript {
    /// 按 `entry_id` 与 `section` 返回分区展开状态，首次查询返回默认值。
    pub fn fold_reveal(&self, entry_id: u64, section: FoldSection) -> FoldReveal {
        self.fold_reveals
            .get(&(entry_id, section))
            .copied()
            .unwrap_or_default()
    }

    /// 【正文】【展开折叠行】为指定条目分区追加十行；`all` 为真时取消上限。
    pub fn reveal_fold(&mut self, entry_id: u64, section: FoldSection, all: bool) {
        let state = self.fold_reveals.entry((entry_id, section)).or_default();
        state.show_all |= all;
        state.extra_lines = state.extra_lines.saturating_add(10);
    }

    /// 收起 `entry_id` 时清除该条目所有分区的展开记录。
    pub(super) fn reset_entry_reveals(&mut self, entry_id: u64) {
        self.fold_reveals.retain(|(id, _), _| *id != entry_id);
    }
}
