#!/usr/bin/env python3
import os
os.chdir(os.path.join("D:", os.sep, "宸铭", "学习", "t大课程", "CSAPP"))
with open('笔记.md', 'r', encoding='utf-8') as f:
    content = f.read()

# Fix: remove the duplicate movzbl/movsbl section in section 6
# Find and replace the entire block
old = """**movzbl vs movsbl**：

| 指令 | 全称 | 高位补什么 |
|------|------|-----------|
| `movzbl` | Move **Zero**-Extend Byte to Long | 补 **0**（无符号扩展） |
| `movsbl` | Move **Sign**-Extend Byte to Long | 补**符号位**（有符号扩展） |

```asm
# %al = 0x9A 时：
movzbl %al, %eax       # "
new = """> 关于 movzbl/movsbl 的详细对比见 §4 的 **mov 家族总结**表。

```asm
# %al = 0x9A 时：
movzbl %al, %eax       # """

if old in content:
    content = content.replace(old, new, 1)
    print("OK")
else:
    print("not found")
    idx = content.find("0x9A")
    if idx > 0:
        print(repr(content[idx-40:idx+60]))

with open('笔记.md', 'w', encoding='utf-8') as f:
    f.write(content)
