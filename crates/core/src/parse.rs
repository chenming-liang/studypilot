//! LLM 输出 JSON 提取共用工具（决策 D4）。
//!
//! 结构化输出共用一条降级链：
//! json_object → prompt 约束 → [`trim_code_fence`] 直解 / [`first_json_block`] 截取
//! → 上层重试一次 → 放弃。本模块提供链路中"从自由文本抠出 JSON"的两个纯函数。

/// 去除包裹的 ``` / ```json 围栏（若有）。
#[must_use]
pub fn trim_code_fence(s: &str) -> &str {
    s.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
}

/// 提取第一个 `{` 到最后一个 `}` 的块（含端点）。
///
/// 粗略但实用：LLM 回复两侧带杂文时，此窗口通常恰好包住完整 JSON 对象；
/// 极端情况下（尾部还有第二个对象/杂文含 `}`）可能截坏，由上层重试兜底。
#[must_use]
pub fn first_json_block(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (end > start).then(|| &s[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_variants_trimmed() {
        assert_eq!(trim_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(trim_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        // 无围栏原样返回（仅去首尾空白）
        assert_eq!(trim_code_fence("  {\"a\":1} "), "{\"a\":1}");
    }

    #[test]
    fn block_extraction() {
        let s = "好的，结果如下：\n{\"title\":\"T\",\"concepts\":[]}\n以上。";
        assert_eq!(
            first_json_block(s),
            Some("{\"title\":\"T\",\"concepts\":[]}")
        );
        assert_eq!(first_json_block("{\"a\":1}"), Some("{\"a\":1}"));
    }

    #[test]
    fn block_extraction_invalid_inputs() {
        assert_eq!(first_json_block("没有花括号"), None);
        assert_eq!(first_json_block("{不闭合"), None);
        // 只有一个字符的情况：'}' 在 '{' 前 → 视为无效
        assert_eq!(first_json_block("}{"), None);
    }
}
