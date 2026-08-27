//! Context Manager：长会话历史裁剪。
//!
//! 长会话把全量 history 发给 LLM 有两个问题：token 成本线性上涨、早期工具结果
//! 污染上下文。本模块以「轮」为原子单位裁剪历史：一轮 = 一条 user 消息 +
//! 其后全部非 user 消息（assistant(+tool_calls) / tool results），
//! 保证 tool_calls 与 tool_result **永不拆散**（拆散会被 API 拒收）。
//!
//! 裁剪策略：从最新轮往回累计字符预算，装不下的最老轮次整体丢弃，
//! 并在保留段最前面插一条 user 角色的省略注记（多数 endpoint 不接受
//! 位于中部的 system 消息，user 角色最稳）。单轮即超预算时至少保留最后一轮。

use crate::message::{Message, Role};

/// 默认字符预算（≈ 8k token 中文的粗略换算）。
pub const DEFAULT_CONTEXT_BUDGET_CHARS: usize = 24_000;

/// 裁剪结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimmedHistory {
    pub messages: Vec<Message>,
    /// 被省略的轮数
    pub omitted_rounds: usize,
    /// 被省略的字符数（消息内容 + tool_calls 参数）
    pub omitted_chars: usize,
}

/// 单条消息的估算长度：正文 + tool_calls 参数（近似 token 成本来源）。
fn message_len(msg: &Message) -> usize {
    msg.content.as_deref().map(str::len).unwrap_or(0)
        + msg
            .tool_calls
            .iter()
            .map(|tc| tc.function.arguments.len())
            .sum::<usize>()
}

/// 把 history 切成轮。user 消息开启新轮；开头若有孤立的非 user 消息
/// （理论上不该出现），归入第 0 轮兜底，保证不丢消息。
fn split_rounds(history: &[Message]) -> Vec<&[Message]> {
    let mut rounds: Vec<&[Message]> = Vec::new();
    let mut start = 0usize;
    for (i, msg) in history.iter().enumerate() {
        if msg.role == Role::User && i > start {
            rounds.push(&history[start..i]);
            start = i;
        }
    }
    if start < history.len() {
        rounds.push(&history[start..]);
    }
    rounds
}

/// 按字符预算裁剪历史。返回的 messages 一定非空（history 非空时至少保留最后一轮）。
#[must_use]
pub fn trim_history(history: &[Message], budget_chars: usize) -> TrimmedHistory {
    let rounds = split_rounds(history);
    let round_lens: Vec<usize> = rounds.iter().map(|r| r.iter().map(message_len).sum()).collect();

    // 从最新轮往回累计预算
    let mut kept_from = rounds.len();
    let mut used = 0usize;
    for (idx, len) in round_lens.iter().enumerate().rev() {
        if used > 0 && used + *len > budget_chars {
            break;
        }
        // 单轮即超预算也保留（used==0 时永不 break），保证至少留最后一轮
        used += *len;
        kept_from = idx;
    }

    if kept_from == 0 {
        return TrimmedHistory {
            messages: history.to_vec(),
            omitted_rounds: 0,
            omitted_chars: 0,
        };
    }

    let omitted_rounds = kept_from;
    let omitted_chars = round_lens[..kept_from].iter().sum();
    let mut messages = Vec::with_capacity(history.len() - omitted_rounds);
    messages.push(Message::user(format!(
        "[对话裁剪提示: 早期 {omitted_rounds} 轮对话（约 {omitted_chars} 字符）\
         已省略以控制上下文长度，与当前问题无关时请忽略]"
    )));
    messages.extend_from_slice(rounds[kept_from..].concat().as_slice());
    TrimmedHistory {
        messages,
        omitted_rounds,
        omitted_chars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    /// 构造一轮：user → assistant(tool_calls) → tool_result → assistant 最终回答
    fn round(q: &str, tool_arg: &str, answer: &str) -> Vec<Message> {
        vec![
            Message::user(q),
            Message::assistant_tool_calls(vec![ToolCall::function(
                "call_1",
                "search_notes",
                format!(r#"{{"query":"{tool_arg}"}}"#),
            )]),
            Message::tool_result("call_1", "x".repeat(500)),
            Message::assistant(answer),
        ]
    }

    #[test]
    fn short_history_untouched() {
        let h = round("问题", "q", "答");
        let t = trim_history(&h, 10_000);
        assert_eq!(t.messages.len(), h.len());
        assert_eq!(t.omitted_rounds, 0);
        assert_eq!(t.omitted_chars, 0);
    }

    #[test]
    fn old_rounds_trimmed_newest_kept() {
        let mut h = Vec::new();
        for i in 0..6 {
            h.extend(round(&format!("问题{i}"), "q", &format!("答{i}")));
        }
        let t = trim_history(&h, 2_000); // 每轮约 530+ 字，2k 只装得下 ~3 轮
        assert!(t.omitted_rounds >= 1 && t.omitted_rounds <= 3, "omitted={t:?}");
        // 最新轮完整保留
        assert_eq!(
            t.messages.last().and_then(|m| m.content.as_deref()),
            Some("答5")
        );
        // 注记在最前面
        assert!(
            t.messages[0]
                .content
                .as_deref()
                .unwrap_or_default()
                .starts_with("[对话裁剪提示")
        );
        // 字符预算大致成立（注记开销除外）
        let used: usize = t.messages[1..].iter().map(message_len).sum();
        assert!(used <= 2_000, "used={used}");
    }

    #[test]
    fn tool_call_pairs_never_split() {
        let mut h = Vec::new();
        for i in 0..5 {
            h.extend(round(&format!("问题{i}"), "q", &format!("答{i}")));
        }
        let t = trim_history(&h, 1_200);
        let msgs = &t.messages[1..]; // 跳过注记
        for (i, m) in msgs.iter().enumerate() {
            if m.role == Role::Assistant && !m.tool_calls.is_empty() {
                // 下一条必须是配对的 tool_result
                let next = msgs.get(i + 1).expect("tool_calls 后必须有 tool_result");
                assert_eq!(next.role, Role::Tool);
                assert_eq!(next.tool_call_id.as_deref(), Some("call_1"));
            }
        }
    }

    #[test]
    fn single_oversized_round_still_kept() {
        let h = round("问题", "q", "答"); // 一轮就超预算
        let t = trim_history(&h, 10);
        assert_eq!(t.omitted_rounds, 0, "至少保留最后一轮");
        assert_eq!(t.messages.len(), 4);
    }

    #[test]
    fn empty_history_ok() {
        let t = trim_history(&[], 1_000);
        assert!(t.messages.is_empty());
        assert_eq!(t.omitted_rounds, 0);
    }
}
