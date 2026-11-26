//! 限量增量读取当前 agy 会话的本地日志，不保留原始内容或思考文本。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct Cursor {
    offset: u64,
    tail: Vec<u8>,
    oversized: bool,
}

impl Cursor {
    /// 【子代理】【读取日志】仅从 `root` 下的固定会话文件读取新字节，返回已完整写入的 JSON 行。
    pub fn read(&mut self, root: &Path, session: &str) -> Vec<Value> {
        let Some(path) = crate::paths::agy_transcript_path(root, session) else {
            return Vec::new();
        };
        // 1. 根目录由调用方规范化，拒绝指向其他会话或非普通文件的链接
        if path.canonicalize().ok().as_deref() != Some(path.as_path()) || !path.is_file() {
            return Vec::new();
        }
        let Ok(mut file) = File::open(path) else {
            return Vec::new();
        };
        let Ok(meta) = file.metadata() else {
            return Vec::new();
        };
        if meta.len() < self.offset {
            *self = Self::default();
        }
        if meta.len() == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        if file.take(128 * 1024).read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset += bytes.len() as u64;
        let mut rows = Vec::new();
        // 2. 保留末尾半行；跳过异常大行而不影响下一条记录
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if !self.oversized && self.tail.len() + part.len() <= 1024 * 1024 {
                self.tail.extend_from_slice(part);
            } else {
                self.oversized = true;
                self.tail.clear();
            }
            if part.last() == Some(&b'\n') {
                if !self.oversized
                    && let Ok(row) = serde_json::from_slice(&self.tail)
                {
                    rows.push(row);
                }
                self.tail.clear();
                self.oversized = false;
            }
        }
        rows
    }
}
