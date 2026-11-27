//! 正文折叠分区、自适应步长与局部读取展开全文状态。

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

impl FoldSection {
    /// 返回当前分区点击「多看一批」时的合理增量步长。
    ///
    /// 返回:
    /// - 单次渐进展开追加的视觉行数
    pub fn reveal_step(self) -> usize {
        match self {
            Self::ToolReadPreview | Self::ToolDiff(_) => 40,
            Self::Thought | Self::ToolOutput | Self::ToolCommand => 20,
            Self::ToolPath => 10,
        }
    }
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

    /// 【正文】【展开折叠行】按分区默认步长追加可见行；`all` 为真时取消上限。
    ///
    /// 参数:
    /// - `entry_id`: 时间线条目 ID
    /// - `section`: 被展开的折叠分区
    /// - `all`: 为真时展开全部剩余行
    pub fn reveal_fold(&mut self, entry_id: u64, section: FoldSection, all: bool) {
        self.reveal_fold_by(entry_id, section, section.reveal_step(), all);
    }

    /// 【正文】【按指定步长展开】为指定条目分区追加 `step` 行；`all` 为真时取消上限。
    ///
    /// 参数:
    /// - `entry_id`: 时间线条目 ID
    /// - `section`: 被展开的折叠分区
    /// - `step`: 本次增量展开行数
    /// - `all`: 为真时展开全部剩余行
    pub fn reveal_fold_by(&mut self, entry_id: u64, section: FoldSection, step: usize, all: bool) {
        let state = self.fold_reveals.entry((entry_id, section)).or_default();
        state.show_all |= all;
        state.extra_lines = state.extra_lines.saturating_add(step);
    }

    /// 判断指定读取条目是否已切换为「查看完整文件」模式。
    ///
    /// 参数:
    /// - `entry_id`: 时间线条目 ID
    ///
    /// 返回:
    /// - 已启用完整文件预览时返回 `true`
    pub fn is_read_full_file(&self, entry_id: u64) -> bool {
        self.full_file_reads.contains(&entry_id)
    }

    /// 【正文】【查看完整文件】将局部读取条目切换为完整本地文件预览，并重置该分区的折叠上限防止超长文件瞬时刷屏。
    ///
    /// 参数:
    /// - `entry_id`: 时间线条目 ID
    pub fn enable_read_full_file(&mut self, entry_id: u64) {
        self.full_file_reads.insert(entry_id);
        self.fold_reveals
            .remove(&(entry_id, FoldSection::ToolReadPreview));
    }

    /// 收起 `entry_id` 时清除该条目所有分区的展开记录与完整文件切换状态。
    pub(super) fn reset_entry_reveals(&mut self, entry_id: u64) {
        self.fold_reveals.retain(|(id, _), _| *id != entry_id);
        self.full_file_reads.remove(&entry_id);
    }
}
