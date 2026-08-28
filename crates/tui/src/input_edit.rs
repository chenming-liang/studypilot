//! 输入框 UTF-8 编辑辅助（纯函数，单测覆盖）。

// ---- 输入框 UTF-8 编辑辅助（纯函数，单测覆盖）----

/// 字符索引 → 字节偏移；越界返回串长。
pub(crate) fn byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(byte, _)| byte)
}

/// 在 pos 处插入字符，返回新光标位置（pos+1）。
pub(crate) fn insert_char(input: &mut String, pos: usize, ch: char) -> usize {
    let pos = pos.min(input.chars().count());
    let byte = byte_offset(input, pos);
    input.insert(byte, ch);
    pos + 1
}

/// 删除 pos 前一个字符，返回新光标位置（pos-1；已在行首则不动）。
pub(crate) fn delete_before(input: &mut String, pos: usize) -> usize {
    if pos == 0 || pos > input.chars().count() {
        return pos.min(input.chars().count());
    }
    let start = byte_offset(input, pos - 1);
    let end = byte_offset(input, pos);
    input.replace_range(start..end, "");
    pos - 1
}

/// 删除 pos 处的字符（光标不动）；pos 越界则不动。
pub(crate) fn delete_at(input: &mut String, pos: usize) {
    if pos >= input.chars().count() {
        return;
    }
    let start = byte_offset(input, pos);
    let end = byte_offset(input, pos + 1);
    input.replace_range(start..end, "");
}
mod tests {
    #[allow(unused_imports)]
    use super::{byte_offset, delete_at, delete_before, insert_char};

    #[test]
    pub(crate) fn byte_offset_ascii_and_cjk() {
        let s = "ab中文cd"; // a b 中 文 c d → 6 字符
        assert_eq!(byte_offset(s, 0), 0);
        assert_eq!(byte_offset(s, 2), 2); // "中" 起点
        assert_eq!(byte_offset(s, 3), 5); // "文" 起点（中占 3 字节）
        assert_eq!(byte_offset(s, 6), 10); // 末尾（总长 1+1+3+3+1+1=10）
        assert_eq!(byte_offset(s, 99), 10); // 越界钳到串长
        assert_eq!(byte_offset("", 0), 0);
    }

    #[test]
    pub(crate) fn insert_char_at_head_middle_tail() {
        let mut s = String::from("ac");
        assert_eq!(insert_char(&mut s, 1, 'b'), 2);
        assert_eq!(s, "abc");

        // 中文插入不破坏 UTF-8
        let mut s = String::from("内存");
        assert_eq!(insert_char(&mut s, 1, '安'), 2);
        assert_eq!(s, "内安存");

        // 头部 / 尾部 / 越界钳制
        let mut s = String::from("bc");
        assert_eq!(insert_char(&mut s, 0, 'a'), 1);
        assert_eq!(s, "abc");
        assert_eq!(insert_char(&mut s, 99, 'd'), 4);
        assert_eq!(s, "abcd");
    }

    #[test]
    pub(crate) fn delete_before_various_positions() {
        // 行首不动
        let mut s = String::from("abc");
        assert_eq!(delete_before(&mut s, 0), 0);
        assert_eq!(s, "abc");

        // 中间删除
        assert_eq!(delete_before(&mut s, 2), 1);
        assert_eq!(s, "ac");

        // 删除中文字符（多字节整体删除，不留残字节）
        let mut s = String::from("内存");
        assert_eq!(delete_before(&mut s, 2), 1);
        assert_eq!(s, "内");

        // 空串 / 越界安全
        let mut s = String::new();
        assert_eq!(delete_before(&mut s, 0), 0);
        assert_eq!(delete_before(&mut s, 5), 0);
    }

    #[test]
    pub(crate) fn delete_at_keeps_cursor() {
        let mut s = String::from("abc");
        delete_at(&mut s, 1);
        assert_eq!(s, "ac");

        // 删除中文
        let mut s = String::from("内存好");
        delete_at(&mut s, 1);
        assert_eq!(s, "内好");

        // 末尾越界不动
        delete_at(&mut s, 2);
        assert_eq!(s, "内好");
        delete_at(&mut s, 99);
        assert_eq!(s, "内好");
    }
}
