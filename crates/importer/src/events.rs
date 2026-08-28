//! 导入进度事件（复用 R4 mpsc 事件通道）。

#[derive(Debug, Clone)]
pub enum ImportEvent {
    Started {
        total: usize,
        course: Option<String>,
    },
    FileStart {
        name: String,
        index: usize,
        total: usize,
    },
    FileDone {
        name: String,
        concepts: usize,
    },
    FileFail {
        name: String,
        err: String,
    },
    FileSkipped {
        name: String,
        /// 去重命中时附存活位置："已存在：课程#id 标题"
        at: Option<String>,
    },
    /// 笔记本体已入库，但 chunk 切片写入失败——该笔记不可检索。
    /// 非致命（不计入 fail），但必须对用户可见以便排查。
    ChunkFail {
        name: String,
        err: String,
    },
    Finished {
        ok: usize,
        skipped: usize,
        fail: usize,
    },
    Cancelled,
}
