# GDB 快速入门

> 配合 CSAPP Lab 使用，重点覆盖 Bomb Lab / Attack Lab 中最常用的操作。

---

## 1. 启动与退出

```bash
gdb bomb                    # 启动 GDB，加载 bomb
gdb -q bomb                 # -q 去掉版本信息，更清爽
gdb -tui bomb               # 启动带文本界面的 GDB（推荐）
```

进入 GDB 后：

```gdb
(gdb) quit                  # 退出
(gdb) q                     # 简写
```

> 💡 建议用 `gdb -tui` 或进入后按 `Ctrl+X+A` 打开 TUI（Text User Interface）模式——上面看汇编/源码，下面输命令，效率高很多。

---

## 2. 运行与断点

### 运行程序

```gdb
(gdb) run                   # 运行（可带参数：run < input.txt）
(gdb) r                     # 简写
(gdb) run < input.txt       # 从文件重定向输入
```

### 下断点

```gdb
(gdb) break main            # 在函数 main 入口下断
(gdb) break phase_1         # 在 phase_1 入口下断
(gdb) break *0x400e00       # 在指定内存地址下断
(gdb) b explode_bomb        # 在 explode_bomb 下断（防止爆炸）
(gdb) b *phase_1+12         # 在 phase_1 入口后偏移 12 字节处
```

> **Bomb Lab 最重要的一条断点**：`b explode_bomb`——先在这里下断，程序就不会真的炸。

#### 能用源码行号下断吗？

有调试符号（编译时加 `-g`）时可以：

```gdb
(gdb) b bomb.c:15           # 在源文件第 15 行下断
(gdb) b main:25             # 在 main 函数的第 25 行下断
(gdb) b 20                  # 在当前文件第 20 行下断
```

但 **Bomb Lab 的 bomb 没有调试符号**，所以 `b bomb.c:15` 会提示 `No source file named bomb.c.`。

替代方案是用函数名或地址：

```gdb
(gdb) b phase_1              # 函数入口
(gdb) b *phase_1+12          # 函数内偏移
(gdb) b *0x400e09            # 直接指定地址
```

判断当前程序有没有调试信息：

```gdb
(gdb) info sources           # 列出有源码信息的文件——空则没有
(gdb) list                   # 显示 "No source file" 就是没有
```

### 查看断点

```gdb
(gdb) info breakpoints      # 列出所有断点
(gdb) info b                # 简写
(gdb) delete 1              # 删除 1 号断点
(gdb) disable 2             # 临时禁用 2 号断点
(gdb) enable 2              # 重新启用
```

---

## 3. 执行控制

```gdb
(gdb) run                   # 从头运行
(gdb) continue              # 继续执行到下一个断点
(gdb) c                     # 简写
(gdb) stepi                 # 执行一条指令（单步）
(gdb) si                    # 简写
(gdb) nexti                 # 执行一条指令，但不进入函数调用
(gdb) ni                    # 简写
(gdb) finish                # 执行完当前函数，打印返回值
(gdb) until                 # 一直执行到当前行/地址之后
```

### si vs s、ni vs n

| 命令 | 粒度 | 一步走多远 |
|------|------|-----------|
| **`si`**（stepi） | 汇编指令 | 执行 `movq $5, %rax` 这样一条指令 |
| **`s`**（step） | C 源码行 | 执行 `x = a + b;` 这一整行 |
| **`ni`**（nexti） | 汇编指令（跳过 call） | 同 si，但不进入函数调用 |
| **`n`**（next） | C 源码行（跳过函数） | 同 s，但不进入函数调用 |

```c
// 一行 C 源码可能编译成多条汇编
int x = a + b;
```

对应汇编可能：

```asm
movq -8(%rbp), %rdi     ← si
addq -12(%rbp), %rdi    ← si
movq %rdi, -16(%rbp)    ← si
                        ← s 直接停在这里（跳过上面 3 条）
```

> **Bomb Lab 中只能用 si / ni**，因为炸弹没有调试符号（没有 `-g`），`s` 和 `n` 没有源码行号可参考。而在你自己写代码并用 `gcc -g` 编译时，`s` 和 `n` 更方便。`si` / `ni` 后面还可以跟数字：`si 3` 执行 3 条指令。


### Bomb Lab 中的典型流程

```gdb
(gdb) b phase_1             # 在 phase_1 入口下断
(gdb) b explode_bomb        # 防止爆炸
(gdb) r < input.txt         # 运行
# 停在 phase_1，开始分析...
(gdb) si                    # 一步步跟进
(gdb) finish                # 看完关键部分，直接跑完 phase_1
(gdb) c                     # 继续到 phase_2...
```

---

## 4. 查看寄存器

```gdb
(gdb) info registers        # 查看所有寄存器
(gdb) info r                # 简写
(gdb) p $rdi                # 打印 %rdi 的值
(gdb) p $rsp                # 打印栈指针
(gdb) p $rax                # 打印返回值寄存器
(gdb) p/x $rdi              # 十六进制显示
(gdb) p/d $rdi              # 十进制显示
(gdb) p/t $rdi              # 二进制显示
(gdb) info r rdi rsi rdx    # 只看指定寄存器
```

### $ 的含义

GDB 中 `$` 用来表示变量，分两种：

**1. 寄存器**：`$rdi`、`$rsp`、`$rax` 等，对应 CPU 的寄存器。注意 GDB 用 `$` 而不是汇编中的 `%`。

```gdb
(gdb) p $rdi         # 打印 %rdi 的值
(gdb) x/s $rsi       # 以 %rsi 为地址去读内存字符串
```

**2. 用户自定义变量**：`$i`、`$count` 等，拿来即用，不用声明类型。

```gdb
(gdb) set $i = 0              # 定义变量 i，初始化为 0
(gdb) p $i                    # → 0
(gdb) set $i = $i + 1         # 自增
(gdb) p $i                    # → 1
```

> 常用于计数：`set $cnt = 0`，每次断点停下后 `set $cnt = $cnt + 1` 来追踪循环执行次数。

### Bomb Lab 实用技巧

```gdb
(gdb) p (char*)$rdi         # 把 %rdi 当作字符串指针打印
(gdb) x/s $rdi              # 同上，更简洁
(gdb) x/s $rsi              # 打印第二个参数字符串
```

> 在 phase_1 的 `strings_not_equal` 调用处，`%rdi` 是你的输入，`%rsi` 就是答案字符串——`x/s $rsi` 直接看答案。

---



## 调普通 C 代码（有源码）

当你自己用 `gcc -g` 编译程序时，GDB 能直接看到源码，调试比汇编级方便很多。编译命令：

```bash
gcc -g -o myprog myprog.c        # -g 保留调试符号
gdb myprog
```

### 按 C 源码行单步

```gdb
(gdb) b myprog.c:15              # 在源码第 15 行下断
(gdb) run
(gdb) s                          # step：进入函数内部
(gdb) n                          # next：执行一行，不进入函数
(gdb) finish                     # 跑完当前函数，返回调用处
```

`s` 和 `n` 按 C 源码行走，不用像无符号程序那样用 `si` 一条条汇编指令走。

### watchpoint（数据断点）

当某个变量的值改变时停下，不用设断点一遍遍 `c`：

```gdb
(gdb) watch x                    # 当 x 的值发生变化时停下
(gdb) rwatch x                   # 当 x 被读取时停下
(gdb) awatch x                   # 当 x 被读写时停下
```

典型场景——找诡异的变量篡改：

```gdb
(gdb) watch buf[i]              # 数组元素被意外修改时触发
(gdb) c                          # 每次触发打印位置，找到罪魁祸首
```

### 条件断点

```gdb
(gdb) b 42 if x != 0             # 只在 x 不为 0 时停在第 42 行
(gdb) b bubble_sort if i > 100   # 循环到第 100 轮才停
```

### 打印表达式和函数调用

```gdb
(gdb) p x + y                    # 计算并打印表达式
(gdb) p strlen(buf)              # 调用函数
(gdb) call my_func(42)           # 直接调用程序中的函数
(gdb) set x = 99                 # 修改变量的值
```

### display（自动打印）

每次断点停下时自动显示某些表达式，不用手动 `p`：

```gdb
(gdb) display x                  # 每次停下自动打印 x
(gdb) display /x i               # 十六进制显示
(gdb) info display               # 查看所有自动打印项
(gdb) undisplay 1                # 删除 1 号 display
```

### backtrace（看调用栈）

程序崩溃时立刻看"怎么走到这的"：

```gdb
(gdb) bt                         # 打印函数调用链
(gdb) frame 2                    # 切换到调用链的第 2 层
(gdb) up                         # 往上一层（调用者）
(gdb) down                       # 往下一层（被调用者）
```
## 5. 查看内存

```gdb
(gdb) x/s 0x400e00          # 以字符串形式查看地址处的内容
(gdb) x/d $rax              # 十进制显示
(gdb) x/4gx $rsp            # 以 8 字节为单位，看栈顶 4 个值
(gdb) x/16b $rdi            # 以 1 字节为单位，看 16 个字节
(gdb) x/10i $rip            # 以指令形式查看当前地址后 10 条指令
(gdb) x/10i 0x400e00        # 查看指定地址的指令
```

**`x` 命令格式**：`x/[数量][大小][格式] <地址>`

写的时候三个参数连在一起不空格，例如 `x/4gx` 表示看 4 个 8 字节的十六进制值。具体各部分含义：

| 格式（格式） | 含义 | 大小（大小） | 含义 |
|------|------|------|------|
| `s` | 字符串 | `b` | 1 字节（byte） |
| `d` | 十进制 | `h` | 2 字节（halfword） |
| `x` | 十六进制 | `w` | 4 字节（word） |
| `i` | 指令 | `g` | 8 字节（giant） |
| `t` | 二进制 |

---

## 6. 反汇编

```gdb
(gdb) disas                 # 反汇编当前函数
(gdb) disas phase_1         # 反汇编 phase_1
(gdb) disas 0x400e00        # 反汇编指定地址附近的代码
(gdb) disas 0x400e00, 0x400e40  # 反汇编指定范围
(gdb) disas /m phase_1      # 混合显示源码和汇编
```

> 💡 **Bomb Lab 配合 objdump 更高效**：`objdump -d bomb > bomb.asm` 拿到完整反汇编文件，用编辑器搜索。GDB 内 `disas` 用于快速查看当前想关注的小段代码即可。

---

## 7. 栈操作

```gdb
(gdb) x/16gx $rsp           # 看栈顶 16 个值（每个 8 字节）
(gdb) x/16gx $rbp           # 从 %rbp 开始看
(gdb) info frame            # 查看当前栈帧信息
(gdb) backtrace             # 查看函数调用链
(gdb) bt                    # 简写
```

---

## 8. 其他实用命令

```gdb
(gdb) list                  # 显示附近源码（如果有调试符号）
(gdb) list phase_1          # 显示 phase_1 的 C 源码
(gdb) set $i = 0            # 定义临时变量
(gdb) p $i++                # 计数用
(gdb) set {int}0x8049000 = 0  # 修改内存
(gdb) call func(arg)        # 直接调用一个函数
(gdb) define mycmd          # 自定义命令
>  commands
> end
```

### 条件断点

```gdb
(gdb) break phase_2 if $rdi == 5   # 仅当 %rdi == 5 时停下
(gdb) watch $rax            # 当 %rax 改变时停下（数据断点）
```


---

[[t大课程/CSAPP/notes/README|返回索引]]
