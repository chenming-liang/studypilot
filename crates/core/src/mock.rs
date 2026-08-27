use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::message::Message;
use crate::provider::{Error, Provider, Response, Result};

/// 按序回放预置响应的 Provider 实现——离线测试的基石。
///
/// 用真实 API 录制的响应构造 fixture 后注入，导入流水线、agent loop
/// 等即可不花一分钱、不碰网络地测全流程。响应耗尽后报错（防测试静默失真）。
pub struct MockProvider {
    inner: Mutex<Inner>,
}

struct Inner {
    responses: Vec<Response>,
    cursor: usize,
    /// 每次 chat 收到的消息快照（供断言 agent loop 回填了什么）。
    received: Vec<Vec<Message>>,
}

impl MockProvider {
    pub fn new(responses: impl IntoIterator<Item = Response>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                responses: responses.into_iter().collect(),
                cursor: 0,
                received: Vec::new(),
            }),
        }
    }

    pub fn one(response: Response) -> Self {
        Self::new([response])
    }

    pub fn remaining(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.responses.len() - inner.cursor
    }

    /// 第 n 次（从 0 起）调用时收到的消息快照。
    pub fn received(&self, call: usize) -> Option<Vec<Message>> {
        self.inner.lock().unwrap().received.get(call).cloned()
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn chat(&self, msgs: &[Message], _tools: &[Value]) -> Result<Response> {
        let mut inner = self.inner.lock().unwrap();
        inner.received.push(msgs.to_vec());
        let resp = inner
            .responses
            .get(inner.cursor)
            .cloned()
            .ok_or_else(|| Error::Config("MockProvider: 预置响应已耗尽".into()))?;
        inner.cursor += 1;
        Ok(resp)
    }
}
