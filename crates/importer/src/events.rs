//! 导入进度事件（复用 R4 mpsc 事件通道）。

/// 单个文件内部的处理阶段（UI 据此显示单文件进度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePhase {
    /// 解析文件（md/pdf/pptx）
    Parsing,
    /// LLM 概念抽取
    Extracting,
    /// 入库
    Inserting,
}

impl FilePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Parsing => "解析",
            Self::Extracting => "抽取概念",
            Self::Inserting => "入库",
        }
    }
}

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
    /// 当前文件的阶段进度（解析/概念抽取/入库）——UI 据此显示单文件进度条
    FileProgress {
        name: String,
        phase: FilePhase,
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
