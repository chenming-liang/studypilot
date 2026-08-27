//! 格式解析器：md/txt / pdf / pptx → 统一 RawDoc { title, text }。
//! 决策 D3：pdf 经 pymupdf 子进程。pptx 用 zip + 正则抽 <a:t>。

use std::io::Read;
use std::path::Path;

use storage::content_hash;

use crate::clean;

/// 统一解析产出。
#[derive(Debug, Clone)]
pub struct RawDoc {
    pub title: String,
    /// content 层：只剥 wikilink，保留原文结构（供展示/引用）
    pub text: String,
    /// FTS 层：额外去目录段+元信息（供检索索引）
    pub fts_text: String,
    #[allow(dead_code)]
    pub content_hash: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("PDF 子进程错误: {0}")]
    Pdf(String),
    #[error("PPTX 解压错误: {0}")]
    PptxZip(String),
    #[error("不支持的文件格式: {0}")]
    Unsupported(String),
}

type Result<T> = std::result::Result<T, ParseError>;

/// 按扩展名分发解析。
pub fn parse_file(path: &Path) -> Result<RawDoc> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "md" | "txt" => parse_markdown(path),
        "pdf" => parse_pdf(path),
        "pptx" => parse_pptx(path),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => Err(ParseError::Unsupported(format!(
            "图片导入（多模态识图）属加分项，本次不支持: {}",
            path.display()
        ))),
        other => Err(ParseError::Unsupported(other.to_owned())),
    }
}

fn parse_markdown(path: &Path) -> Result<RawDoc> {
    let raw = std::fs::read_to_string(path)?;
    let title = clean::extract_title(&raw).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_owned()
    });
    let text = clean::clean_for_content(&raw);
    let fts_text = clean::clean_for_fts(&raw);
    let hash = content_hash(&text);
    Ok(RawDoc {
        title,
        text,
        fts_text,
        content_hash: hash,
    })
}

/// pdf：经 scripts/pdf_extract.py 子进程提取。决策 D3。
fn parse_pdf(path: &Path) -> Result<RawDoc> {
    let script = find_script();
    let output = std::process::Command::new("python3")
        .arg(script)
        .arg(path)
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .map_err(|e| ParseError::Pdf(format!("启动 python3 失败: {e}（确保已安装 pymupdf）")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(ParseError::Pdf(err.to_string()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ParseError::Pdf(format!("解析 JSON 失败: {e}")))?;

    let pages: Vec<String> = json
        .get("page_texts")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let text = pages.join("\n\n");
    if text.trim().is_empty() {
        return Err(ParseError::Pdf("提取文本为空（疑似扫描版 PDF）".into()));
    }

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_owned();
    let hash = content_hash(&text);
    Ok(RawDoc {
        title,
        text: text.clone(),
        fts_text: text,
        content_hash: hash,
    })
}

/// pptx：zip 解压 → 读 ppt/slides/slide*.xml → 抽 <a:t> 文本。
fn parse_pptx(path: &Path) -> Result<RawDoc> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| ParseError::PptxZip(e.to_string()))?;

    let mut slides: Vec<(usize, String)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ParseError::PptxZip(e.to_string()))?;
        let name = entry.name().to_owned();
        // 匹配 ppt/slides/slideN.xml
        if let Some(n) = name
            .strip_prefix("ppt/slides/slide")
            .and_then(|s| s.strip_suffix(".xml"))
            .and_then(|s| s.parse::<usize>().ok())
        {
            let mut xml = String::new();
            let mut entry = entry;
            entry.read_to_string(&mut xml).map_err(ParseError::from)?;
            slides.push((n, extract_a_t_text(&xml)));
        }
    }

    if slides.is_empty() {
        return Err(ParseError::PptxZip("PPTX 中未找到 slide 文件".into()));
    }

    slides.sort_by_key(|(n, _)| *n);
    let text = slides
        .iter()
        .map(|(n, t)| format!("--- 第 {n} 页 ---\n{t}"))
        .collect::<Vec<_>>()
        .join("\n\n");

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_owned();
    let hash = content_hash(&text);
    Ok(RawDoc {
        title,
        text: text.clone(),
        fts_text: text,
        content_hash: hash,
    })
}

/// 从 slide XML 中提取所有 <a:t>...</a:t> 标签文本。
fn extract_a_t_text(xml: &str) -> String {
    let re = regex::Regex::new(r"<a:t>([^<]*)</a:t>").unwrap();
    re.captures_iter(xml)
        .map(|c| c.get(1).unwrap().as_str().to_owned())
        .collect::<Vec<_>>()
        .join("")
}

/// 定位 scripts/pdf_extract.py：exe 同级 → workspace 根 → CARGO_MANIFEST_DIR。
fn find_script() -> std::path::PathBuf {
    let candidates = [
        std::path::PathBuf::from("scripts/pdf_extract.py"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pdf_extract.py"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    candidates[0].clone()
}
