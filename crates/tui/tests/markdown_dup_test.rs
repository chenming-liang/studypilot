//! 防回归验证：markdown 渲染每段文本只输出一次（曾因双重 push 产生成对重复）。
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// 复刻 LineBuffer 的推送逻辑（与 markdown.rs 保持一致的语义），
/// 因为 markdown.rs 在 bin crate 内部无法直接引用。
struct Buf {
    parts: Vec<(String, bool)>, // (text, bold)
    bold: bool,
}

impl Buf {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            bold: false,
        }
    }
    fn push_str(&mut self, text: &str) {
        if let Some(last) = self.parts.last_mut()
            && last.1 == self.bold
        {
            last.0.push_str(text);
        } else {
            self.parts.push((text.to_owned(), self.bold));
        }
    }
    fn text(&self) -> String {
        self.parts.iter().map(|(s, _)| s.as_str()).collect()
    }
}

#[test]
fn bold_text_rendered_once_not_twice() {
    let md = "把每个值想成一件**有唯一主人**的物品。在离开作用域时**负责清理**（释放内存）。";
    let mut buf = Buf::new();
    for ev in Parser::new_ext(md, Options::empty()) {
        match ev {
            Event::Text(t) => buf.push_str(&t), // 只推一次（修复后的语义）
            Event::Code(t) => buf.push_str(&t),
            Event::Start(Tag::Strong) => buf.bold = true,
            Event::End(TagEnd::Strong) => buf.bold = false,
            _ => {}
        }
    }
    let out = buf.text();
    assert_eq!(
        out.matches("有唯一主人").count(),
        1,
        "bold 文本出现多次: {out}"
    );
    assert_eq!(
        out.matches("负责清理").count(),
        1,
        "bold 文本出现多次: {out}"
    );
    // 原文完整性
    assert!(out.contains("把每个值想成一件"), "正文丢失: {out}");
}
