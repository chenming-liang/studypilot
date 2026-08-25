# CSAPP 暑假自学计划

> **学习者**：清华大学 交叉信息学院（姚班）大一学生  
> **时间**：2026 年暑假（计划 8 周，约 200-240 小时）  
> **目标**：通读全书 12 章 + 完成全部 Lab

---

## 目录

1. [课程简介](#1-课程简介)
2. [先修情况评估](#2-先修情况评估)
3. [学习资料清单与获取方式](#3-学习资料清单与获取方式)
4. [环境搭建](#4-环境搭建)
5. [总体时间规划](#5-总体时间规划)
6. [分章详细计划](#6-分章详细计划)
7. [Lab 详解](#7-lab-详解)
8. [每日建议节奏](#8-每日建议节奏)
9. [给竞赛生的特别提醒](#9-给竞赛生的特别提醒)
10. [检查清单](#10-检查清单)

---

## 1. 课程简介

**CSAPP** = *Computer Systems: A Programmer's Perspective*（《深入理解计算机系统》）

这是 CMU 的经典课程 **15-213**，被誉为"程序员必读圣经"。它不教你写代码，而是教你**代码在计算机上到底是怎么跑的**——从 C 代码到汇编，到 CPU 指令执行，到内存层级，到操作系统交互。

### 你将收获

- 理解程序的完整生命周期：`C → ASM → 二进制 → 执行`
- 写出**缓存友好**、**可预测性能**的代码
- 理解缓冲区溢出、ROP 等底层攻击原理
- 自己实现 `malloc`/`free` 内存分配器
- 掌握并发编程的基本范式
- **为后续课程打下坚实基础**：操作系统、编译原理、体系结构、计算机网络

---

## 2. 先修情况评估

| 维度 | 现状 | 评估 |
|------|------|------|
| **C 语言** | 竞赛背景，熟悉 C/C++ | ✅ 语法层面足够，但需适应"工程写法"（指针、结构体、文件操作） |
| **数学** | 已修数学分析、高等线性代数 | ✅ 完全覆盖 CSAPP 所需数学 |
| **体系结构** | 全新领域 | ⚠️ 这是最大挑战，从零开始 |
| **操作系统** | 全新领域 | ⚠️ Ch8 之后会涉及 |
| **开发环境** | Windows + MinGW GCC/GDB | ⚠️ 需要搭建 Unix 环境 |

---

## 2.5 当前进度与 Lab 解锁

> 以下进度以 `notes/` 中已整理的章节笔记为准（每篇对应一章，链接指向 `notes/README.md` 索引）。笔记 = 该章讲义已整理完毕，可以据此开做对应 Lab。

### 笔记已覆盖的章节

| 章节 | 笔记 | Lab 解锁 |
|------|------|---------|
| Ch2 信息的表示与处理 | `第2章-信息的表示与处理` | ✅ **Data Lab** |
| Ch3 程序的机器级表示 | `第3章-程序的机器级表示`（含汇编、过程调用、**缓冲区溢出/防御/ROP**） | ✅ **Bomb Lab / Attack Lab** |
| Ch5 程序优化 | `第5章-程序优化` | — |
| Ch6 存储器层级 | `第6章-存储器层级`（含缓存组织 S/E/B、**矩阵乘法与分块**） | ✅ **Cache Lab** |
| Ch7 链接 | `第7章-链接` | — |
| Ch8 异常控制流 | `第8章-异常控制流`（进程、系统调用、**信号**） | ✅ **Shell Lab** |
| Ch9 虚拟内存 | `第9章-虚拟内存` | — |
| Ch9.9 动态内存分配 | `第9章-动态内存分配`（隐式/显式/分离链表） | ✅ **Malloc Lab** |
| Ch10 系统级 I/O | `第10章-系统级IO` | Shell Lab 配套 |
| Ch12 并发编程 | `第12章-并发编程`（含进程/事件/线程并发服务器） | Proxy Lab 的并发部分 ✅ |

### 尚未覆盖的章节

| 章节 | 影响 |
|------|------|
| Ch1 系统漫游 | 无对应 Lab，可略 |
| **Ch4 处理器体系结构** | **Arch Lab 依赖**（选做，需自读教材） |
| **Ch11 网络编程** | **Proxy Lab 依赖**；`第12章-并发编程` 的 socket/echo 服务器可作为起步，但建议补 Ch11 |

### Lab 解锁状态一览

| Lab | 前置章节 | 你的状态 |
|-----|---------|---------|
| **Data Lab** | Ch2 | ✅ 已完成（`实验-Data Lab`） |
| **Bomb Lab** | Ch3 | 🟢 **现在可做** |
| **Attack Lab** | Ch3（溢出/ROP） | ✅ 已完成 Level 2（`实验-Attack Lab`） |
| **Cache Lab** | Ch6 | 🟢 **现在可做** |
| **Shell Lab** | Ch8 + Ch10 | 🟢 **现在可做** |
| **Malloc Lab** | Ch9.9 | 🟢 **现在可做** |
| **Proxy Lab** | Ch11 + Ch12 | 🟡 并发部分已备，**补 Ch11 后做** |
| **Arch Lab** | Ch4 | ⚪ 选做；需补 Ch4 |

### 一句话回答："学到哪里可以去做 Lab"

> **现在（笔记已覆盖到 Ch12）**：Ch2→Data Lab✅、Ch3→Bomb/Attack Lab✅、Ch6→Cache Lab、Ch8+Ch10→Shell Lab、Ch9.9→Malloc Lab 全部解锁。
> **建议的下一步开做顺序**：**Bomb Lab → Cache Lab → Shell Lab → Malloc Lab**（对应章节笔记齐全，边做边巩固）。
> **还差一步的**：Proxy Lab 需补 **Ch11 网络编程**（约 4–6h 教材，`第12章` 并发服务器已打底）；Arch Lab 为选做，需自补 Ch4。

---

## 3. 学习资料清单与获取方式

### 3.1 教材（必读）

| 资料 | 说明 | 获取方式 |
|------|------|----------|
| **《Computer Systems: A Programmer's Perspective》3rd Edition** | 原版教材，必读 | [官网](http://csapp.cs.cmu.edu/) 可购买或下载样章 |
| **《深入理解计算机系统》第三版中文版** | 中文翻译，可对照 | 微信读书、京东/当当购书，或图书馆借阅 |
| **CSAPP 官方资源页面** | 勘误表、PPT、代码示例 | http://csapp.cs.cmu.edu/3e/home.html |

> ⚠️ 中文版翻译质量一般，部分术语不一致。**建议以英文版为主，中文版作为遇到难处时的参考**。

### 3.2 视频课程（推荐）

| 资源 | 说明 | 获取方式 |
|------|------|----------|
| **CMU 15-213 2015 版** | 最经典的录制版本，Bryant/O'Hallaron 亲自授课 | B 站搜索 "CMU 15-213" 或 YouTube CMU 官方频道 |
| **CMU 15-213 最新学期** | 每年更新，内容最新 | YouTube 搜索 "15-213 cmu" 按年份筛选 |
| **中文翻译字幕版** | B 站 UP 主翻译 | B 站搜索 "深入理解计算机系统 中文翻译" |

> 💡 **使用建议**：每章阅读前先看对应的 lecture 视频（1.25-1.5x 速），建立整体认知，再细读教材。

### 3.5 各 Lab 与视频的对应关系 🎬


根据提供的图片（image_5090da.png、image_5090fb.png、image_50911a.png、image_509137.png），完整的课程列表（共26课）提取如下：

- **Lecture 01** Course Overview (01:15:09)
    
- **Lecture 02** Bits, Bytes, and Integers (01:11:05)
    
- **Lecture 03** Bits, Bytes, and Integers (01:16:35)
    
- **Lecture 04** Floating Point (01:11:33)
    
- **Lecture 05** Machine Level Programming (01:13:29)
    
- **Lecture 06** Machine Level Programming (01:13:50)
    
- **Lecture 07** Machine Level Programming (01:06:45)
    
- **Lecture 08** Machine Level Programming (01:19:45)
    
- **Lecture 09** Machine Level Programming (01:18:49)
    
- **Lecture 10** Program Optimization (01:13:38)
    
- **Lecture 11** The Memory Hierarchy (01:15:47)
    
- **Lecture 12** Cache Memories (01:18:43)
    
- **Lecture 13** Linking (01:21:33)
    
- **Lecture 14** Exceptional Control Flow (01:12:09)
    
- **Lecture 15** Exceptional Control Flow (01:19:27)
    
- **Lecture 16** System Level I/O (01:11:01)
    
- **Lecture 17** Virtual Memory Concepts (01:11:14)
    
- **Lecture 18** Virtual Memory Systems (01:17:48)
    
- **Lecture 19** Dynamic Memory Allocation (01:06:28)
    
- **Lecture 20** Dynamic Memory Allocation (01:22:43)
    
- **Lecture 21** Network Programming (01:17:03)
    
- **Lecture 22** Network Programming (01:18:00)
    
- **Lecture 23** Concurrent Programming (01:11:38)
    
- **Lecture 24** Synchronization Basics (56:31)
    
- **Lecture 25** Synchronization Advanced (01:20:43)
    
- **Lecture 26** Thread Level Parallelism (01:05:36)
    

_(注：部分因界面显示省略号的课程名称，已根据上下文及该课程（CS:APP / 15-213）的标准大纲进行了完整补全。)_

在 B 站上看 CMU 15-213 2015 版视频时，以下映射告诉你**学到第几讲就可以开始做对应的 Lab**。注意这是按你提取的实际课程列表校对过的：

| Lab | 需看完的 Lec | 累计进度 | 对应教材章节 | 视频内容 |
|-----|-------------|---------|-------------|---------|
| **Data Lab** | 02~04 | 第 4 讲 | Ch2 | Lec02~03 Bits/Bytes/Integers → Lec04 Floating Point |
| **Bomb Lab** | 05~07 | 第 7 讲 | Ch3(1-7) | Lec05 汇编基础 → Lec06 分支循环 → Lec07 栈帧 |
| **Attack Lab** | 08~09 | 第 9 讲 | Ch3(8-10) | Lec08 数组/结构体 → Lec09 溢出/ROP |
| **Architecture Lab** | (教材 Ch4) | — | Ch4 | 自学教材 Ch4，不依赖视频 |
| **Cache Lab** | 11~12 | 第 12 讲 | Ch6 | Lec11 Memory Hierarchy → Lec12 Cache Memories |
| **Shell Lab** | 14~15 | 第 15 讲 | Ch8 | Lec14~15 Exceptional Control Flow（进程+信号） |
| **Malloc Lab** | 19~20 | 第 20 讲 | Ch9.9 | Lec19~20 Dynamic Memory Allocation |
| **Proxy Lab** | 21~25 | 第 25 讲 | Ch11~12 | Lec21~22 Network Programming → Lec23~25 Concurrency/Sync |

> ⚠️ **视频序号注意**：CMU 课程顺序与教材章节并非一一对应。如果按 CMU 视频顺序看，**自然的 Lab 做顺序是**：
>
> **Data Lab → Bomb Lab → Attack Lab → Architecture Lab → Cache Lab → Shell Lab → Malloc Lab → Proxy Lab**

#### 各 Lab 开做时机详解

**Data Lab**（看完 Lec 02~04）
- Lec 02~03 的 Bits/Bytes/Integers 覆盖了补码和位运算基础，看完就可以做 Data Lab 的整数题。
- Lec 04 专门讲 Floating Point（IEEE 754），看完后做浮点题。
- **最佳开做时机**：Lec 04 结束。

**Bomb Lab**（看完 Lec 05~07）
- Lec 05 开始进入 Machine Level Programming，覆盖寄存器、指令、寻址模式。
- Lec 06 讲条件码、分支、循环。
- Lec 07 讲栈帧和函数调用约定，这是反汇编的必备知识。
- **最佳开做时机**：Lec 07 结束。

**Attack Lab**（看完 Lec 08~09）
- Machine Level Programming 共 5 讲（Lec 05~09），后两讲覆盖数组/结构体分配（Lec 08）和缓冲区溢出/ROP（Lec 09）。
- **最佳开做时机**：Lec 09 结束。

**Cache Lab**（看完 Lec 11~12）
- Lec 10 是 Program Optimization（Ch5），与 Cache Lab 无关，可以看也可以跳过。
- Lec 11 覆盖 Memory Hierarchy（SRAM/DRAM/局部性原理）→ 可以做 Part A。
- Lec 12 专门讲 Cache 和分块优化 → 可以做 Part B。
- **最佳开做时机**：Lec 12 结束。

**Shell Lab**（看完 Lec 14~15，选做）
- Lec 14~15 Exceptional Control Flow 覆盖异常、进程控制（fork/exec/wait）、信号处理。
- Lec 16 是 System Level I/O（Ch10），与 Shell Lab 无关。
- **最佳开做时机**：Lec 15 结束。

**Malloc Lab**（看完 Lec 19~20）
- Lec 17~18 是 Virtual Memory（Ch9 前 8 节），建议先看教材理解页表和地址翻译。
- Lec 19~20 专门讲 Dynamic Memory Allocation（隐式空闲链表 → 显式链表/分离适配）。
- **最佳开做时机**：Lec 20 结束。

**Proxy Lab**（看完 Lec 21~25）
- Lec 21~22 讲 Network Programming（socket/HTTP）。
- Lec 23~25 讲 Concurrent Programming 和 Synchronization（线程/信号量/锁）。
- **最佳开做时机**：Lec 25 结束。

#### 视频观看顺序示意图

```
Lec 01 ─→ Lec 02 ─→ Lec 03 ─→ Lec 04 ─→ Lec 05 ─→ Lec 06 ─→ Lec 07
（概论）   （位运算）  （整续）   （浮点）   （汇编基础） （分支循环）  （栈帧）
                                  ↓                        ↓
                              Data Lab                  Bomb Lab

Lec 08 ─→ Lec 09 ─→ Lec 10 ─→ Lec 11 ─→ Lec 12 ─→ Lec 13
（数组）   （溢出/ROP） （优化）  （内存层次） （缓存）   （链接）
  ↓          ↓                            ↓
Attack Lab                         Cache Lab

Lec 14 ─→ Lec 15 ─→ Lec 16 ─→ Lec 17 ─→ Lec 18 ─→ Lec 19 ─→ Lec 20
（ECF）   （进程/信号）（I/O）  （虚拟内存） （虚拟内存续）（内存分配）（分配器进阶）
  ↓                                                       ↓
Shell Lab                                              Malloc Lab

Lec 21 ─→ Lec 22 ─→ Lec 23 ─→ Lec 24 ─→ Lec 25 ─→ Lec 26
（网络）   （HTTP）   （并发）   （同步基础） （同步进阶） （线程级并行）
              ↓                               ↓
          Proxy Lab ◄───────────────────────┘
```

> 💡 **推荐做法**：在 B 站上打开播放列表，按 Lec 顺序看。每看完 2-3 讲就停下来做对应的 Lab，不要一口气刷完所有视频再开始动手。

### 3.3 Lab（重中之重）

| Lab | 获取方式 |
|-----|----------|
| 所有 Lab 源码 | http://csapp.cs.cmu.edu/3e/labs.html |
| **自学者说明** | Lab 官方需要教师申请才能获得完整评测脚本。**自学者可以从 GitHub 搜索 "csapp lab" 找到开源实现和测试框架**，或使用第三方维护的版本（如 [Small-Pond/CSAPP-Labs](https://github.com/Small-Pond/CSAPP-Labs) 等社区仓库）。注意评测脚本的版权归属，仅供个人学习使用。 |

### 3.4 辅助工具

| 工具 | 用途 | 获取方式 |
|------|------|----------|
| **godbolt.org (Compiler Explorer)** | 在线查看 C→汇编，学 Ch3 必备 | https://godbolt.org/ |
| **GDB 速查表** | 调试反汇编代码 | 搜索 "GDB cheat sheet" |
| **Visual Studio Code** | 代码编写 | https://code.visualstudio.com/ |
| **Draw.io / Excalidraw** | 画内存布局、cache 图辅助理解 | https://excalidraw.com/ |

---

## 4. 环境搭建

### 4.1 为什么需要 Unix 环境

CSAPP 的 Lab 全部基于 **Linux/gcc** 编写，评测脚本也假定 Unix 环境。Windows 的 MinGW 在以下 Lab 会出现问题：

- **Shell Lab**：fork/exec/signal 行为差异
- **Proxy Lab**：socket 编程 API 差异
- **Malloc Lab**：mmap 行为差异
- **官方评测脚本 `driver.py`**：依赖 Python Unix 特性

### 4.2 推荐方案：WSL2（最省事）

```bash
# 1️⃣ 管理员权限打开 PowerShell/CMD，安装 WSL2
wsl --install -d Ubuntu-22.04

# 2️⃣ 重启后进入 WSL，安装开发工具
sudo apt update && sudo apt upgrade -y
sudo apt install -y build-essential gdb valgrind python3 python3-pip

# 3️⃣ 确认安装
gcc --version        # 应显示 gcc 11.x+
gdb --version        # 应显示 GDB
make --version       # 应显示 GNU Make
valgrind --version   # 应显示 Valgrind（内存检测，Malloc Lab 必用）

# 4️⃣ 在 WSL 中创建 CSAPP 工作目录
mkdir -p ~/csapp-labs
cd ~/csapp-labs
```

### 4.3 备选方案：MSYS2（Windows 原生，不推荐）

如果实在不想装 WSL2，可用 MSYS2：

```bash
# 在 MSYS2 终端中
pacman -S mingw-w64-x86_64-gcc mingw-w64-x86_64-gdb make
```

但 Shell/Proxy Lab 仍可能在 MSYS2 下遇到问题。

### 4.4 代码编辑器推荐

在 WSL 下，推荐使用 VSCode + Remote-WSL 插件：

1. 安装 VSCode
2. 安装 "Remote - WSL" 插件
3. 在 WSL 中运行 `code .` 即可打开
4. 推荐安装 C/C++ 扩展包

---

## 5. 总体时间规划

### 时间分配

```
第一阶段：通读全书 + 前 5 个 Lab（第 1-5 周，~140h）
第二阶段：高级 Lab + 总复习（第 6-8 周，~80h）
```

### 视频与 Lab 的并行脉络

除了看教材，你还要按顺序看 CMU 15-213 视频。以下是**视频 → Lab 的并行路线**：

```
周次  视频进度                                         Lab
W1    Lec 01(概论) → Lec 02(位运算)                      
      → Lec 03(整数续) → Lec 04(浮点)                   Data Lab
W2    Lec 05(汇编基础) → Lec 06(分支循环)                 
      → Lec 07(栈帧)                                     Bomb Lab
W3    Lec 08(数组/结构体) → Lec 09(溢出/ROP)            Attack Lab
      Ch4(处理器体系结构)                             Architecture Lab
      Lec 10(程序优化) → Lec 11(内存层次)                 
      → Lec 12(缓存)                                     Cache Lab
W4    Lec 13(链接) → Lec 14(ECF)                         
      → Lec 15(进程/信号)                                 Shell Lab（选做）
W5    Lec 16(系统I/O) → Lec 17~18(虚拟内存)              阅读教材 Ch9
W6    Lec 19(动态分配) → Lec 20(分配器进阶)              Malloc Lab
W7    Lec 21~22(网络编程) → Lec 23(并发)                 
      → Lec 24(同步基础) → Lec 25(同步进阶)              Proxy Lab
W8    Lec 26(线程级并行)                                  Proxy Lab（续）
```

---

## 6. 分章详细计划

> 💡 **视频与 Lab 的配合方式**：每章的"视频"栏标注了对应 CMU 15-213 Lecture 编号。**建议按 Lec 顺序看视频，看完对应 Lec 后立即做 Lab**，不要屯着 Lab 到最后做。具体每个 Lab 对应哪些 Lec 见 [3.5 节](#35-各-lab-与视频的对应关系)。

### 第 1 周：计算机系统概览 + 数据表示

#### Ch1 - 计算机系统漫游

- **阅读**：1h
- **内容**：以 `hello.c` 为例，追踪从源程序到执行的完整过程
- **要点**：
  - 理解 Amdahl 定律（加速比公式）
  - 建立"系统栈"的整体图景：CPU → 内存 → OS → I/O
- **习题**：不做深入，浏览即可
- **视频**：CMU 15-213 Lecture 01 (Great Ideas)

#### Ch2 - 信息的表示和处理 ⭐⭐⭐

- **阅读**：6-8h
- **内容**：整数编码（原码/反码/补码）、整数运算（溢出、移位）、浮点数 IEEE 754
- **要点**：
  - 补码的特性：`-x = ~x + 1`
  - 整数运算的溢出不会 crash，但浮点会
  - 浮点精度问题：`(x + y) + z ≠ x + (y + z)`
- **习题**：**必做** 2.11-2.29（基础）、2.31-2.42（进阶）、2.47-2.56（浮点）、2.57-2.78（挑战）
- **视频**：CMU 15-213 Lecture 02~04 (Bits, Bytes, Integers & Floating Point)

#### 🧪 Lab：Data Lab

- **时间**：8-15h
- **任务**：用有限的位运算符实现 `bitXor`、`isLessOrEqual`、`floatScale2` 等函数
- **核心技能**：位运算、补码运算、IEEE 754 格式
- **关键技巧**：
  - 利用 `x & (x-1)` 清除最低位的 1
  - 检查符号位：`x >> 31` 或 `!(x ^ 0x80000000)`
  - 对于浮点操作，善用 `unsigned` 和 `int` 的类型转换
  - 先理解 `float` 的位布局：1 位符号 + 8 位指数 + 23 位尾数
- **⚠️ 竞赛生命题**：你的位运算直觉会很强，但注意这道题考察的是**对编码规范的理解**，不是炫技。理解每个运算符为什么被限制，比通过测试更重要。

---

### 第 2 周：汇编基础

#### Ch3 - 程序的机器级表示（1-7 节）

- **阅读**：10-12h
- **内容**：x86-64 寄存器、AT&T 汇编语法、条件码、循环、函数调用栈帧
- **要点**：
  - **AT&T vs Intel 语法**：记住 `mov src, dst`（源在左）是 AT&T；Intel 是 `mov dst, src`
  - 寄存器命名：`%rax`(64b) / `%eax`(32b) / `%ax`(16b) / `%al`(8b)
  - 函数调用 6 个参数寄存器：`%rdi, %rsi, %rdx, %rcx, %r8, %r9`
  - 栈帧布局：caller 的返回地址 → 被保存在 rsp 指向的位置
  - **叶函数优化**：不需要栈帧，全用寄存器
- **实践**：
  ```bash
  # 写一段 C 代码，反汇编看效果
  cat > test.c << 'EOF'
  int add(int a, int b) { return a + b; }
  int main() { return add(1, 2); }
  EOF
  gcc -Og -S test.c   # 生成汇编 test.s
  gcc -Og -c test.c   # 生成目标文件，再用 objdump
  objdump -d test.o   # 反汇编
  ```
- **习题**：3.1-3.10（基本语法）、3.11-3.20（条件/循环）、3.21-3.30（函数调用）
- **视频**：CMU 15-213 Lecture 05~07 (Machine Level Programming I~III)

#### 🧪 Lab：Bomb Lab

- **时间**：6-10h
- **任务**：反汇编一个"炸弹"程序，通过 GDB 分析绕过 6 个 phase
- **核心技能**：GDB 基本命令、x86-64 汇编阅读、逆向思维
- **关键技巧**：
  - `gdb bomb` → `layout asm` 查看反汇编
  - `break *0x地址` 在任意地址下断点
  - `info registers` 查看寄存器状态
  - `x/s $rdi` 查看字符串参数
  - 重点关注 `strings_not_equal` 等字符串比较函数
  - 善用 `stepi`(单步指令) 和 `finish`(执行到函数返回)
- **⚠️ 不要直接搜答案**：Bomb Lab 的乐趣在于自己逆向。实在卡住时，先读对应的 3.7-3.10 节。

---

### 第 3 周：汇编进阶 + 性能 + 缓存

#### Ch3（8-10 节）

- **阅读**：4-6h
- **内容**：数组分配与访问、异质数据结构（struct/union）、缓冲区溢出攻击、ROP
- **要点**：
  - 数组访问：`A[i]` → `*(A + i * sizeof(elem))`
  - struct 对齐：最大字段对齐（如内含 double 则 8 字节对齐）
  - 缓冲区溢出：gets/strcpy 不检查边界
  - ROP 原理：用栈上已有的 gadget 拼接攻击链
- **习题**：3.31-3.40（数组）、3.41-3.50（struct）、3.51-3.58（溢出）

#### Ch5 - 优化程序性能

- **阅读**：3-4h
- **内容**：编译器优化的限制、循环展开、SIMD 概念、性能分析
- **要点**：
  - **延迟 vs 吞吐量**：延迟是单次指令用时，吞吐是单位时间能执行的次数
  - 循环展开：减少控制开销，增加指令级并行
  - 关键路径：找出循环体的关键路径长度，这就是理论下界
  - `__builtin_expect`：分支预测提示
- **习题**：5.1-5.8（基础）、5.9-5.15（进阶）

#### Ch6 - 存储器层次结构

- **阅读**：4-6h
- **内容**：SRAM/DRAM、局部性原理、缓存结构（直接映射/组相联/全相联）
- **要点**：
  - **空间局部性 vs 时间局部性**
  - 缓存参数公式：`(块地址) → tag | set index | block offset`
  - 写策略：直写(write-through) vs 写回(write-back)；写分配 vs 非写分配
  - **矩阵乘法的缓存优化**：分块（tiling）
- **习题**：6.1-6.8（缓存基础）、6.9-6.18（地址映射）、6.19-6.25（矩阵分析）
- **视频**：CMU 15-213 Lecture 11 (Cache Memory) & Lecture 12 (Cache Performance)

#### 🧪 Lab：Attack Lab

- **时间**：8-12h
- **任务**：对给定程序实施缓冲区溢出攻击（Code Injection + ROP）
- **核心技能**：栈布局理解、ROP gadget 利用
- **关键技巧**：
  - `farm.c` 是 ROP 的 gadget 来源
  - `hex2raw` 将十六进制输入转化为原始字节
  - 第一阶段（CI）：在栈上放 shell code 的地址
  - 第二阶段（ROP）：利用 `popq %rax; ret` 等 gadget 设置参数

#### 🧪 Lab：Cache Lab

- **时间**：10-15h
- **任务**：Part A 编写缓存模拟器；Part B 优化矩阵转置的缓存命中率
- **核心技能**：缓存结构理解、分块优化技巧
- **关键技巧**：
  - Part A：用 LRU 替换策略，用 struct 表示 cache line
  - Part B：**分块大小 8×8 或 16×16**，注意对角线上的冲突失效
  - 利用局部性：按块访问，而不是按行/列
  - 对 32×32 矩阵：8×8 分块很容易满分
  - 对 64×64 矩阵：需要用 4×4 或 8×8 分块 + 对角线技巧
  - 对 61×67 非规整矩阵：需要更复杂的策略

---

#### 🧪 Lab：Architecture Lab

- **时间**：10-15h（选做，教材 Ch4 配套 Lab）
- **任务**：用 Y86-64（简化版 x86-64）汇编编写程序，并修改流水线处理器的硬件描述
  - Part A：用 Y86-64 汇编实现 `sum`、`rsum` 等函数
  - Part B：为 SEQ 处理器添加 `iaddq` 指令
  - Part C（挑战）：优化 PIPE 流水线，提高 C 程序的运行效率
- **核心技能**：Y86-64 指令集、流水线结构、转发、冒险检测
- **依赖关系**：需要读完 **Ch4（处理器体系结构）**。不依赖视频课程（CMU 15-213 没有对应的 Lec 讲 Ch4）
- **选做**：如果对 CPU 如何执行指令不感兴趣，可以跳过直接进 Cache Lab，不影响后续 Lab

---

### 第 4 周：链接 + 异常控制流

#### Ch7 - 链接

- **阅读**：5-7h
- **内容**：ELF 目标文件、符号表、符号解析、重定位、动态链接、PIC
- **要点**：
  - ELF 关键 section：`.text`(代码) `.data`(已初始化全局) `.bss`(未初始化全局) `.symtab`(符号表)
  - 符号解析规则：强符号覆盖弱符号（同名全局变量坑！）
  - 重定位类型：`R_X86_64_PC32`(相对) vs `R_X86_64_32`(绝对)
  - 共享库：位置无关代码（PIC）通过 GOT/PLT 实现
- **习题**：7.1-7.6（符号解析）、7.7-7.12（重定位）
- **视频**：CMU 15-213 Lecture 13 (Linking)

#### Ch8 - 异常控制流

- **阅读**：6-8h
- **内容**：异常、进程上下文切换、fork/exec/wait、信号处理、setjmp/longjmp
- **要点**：
  - **fork 的货架模型**：fork 一次，返回两次
  - `execve` 加载新程序，不返回（失败才返回 -1）
  - **僵死进程**：子进程先结束，父进程还没 wait
  - 信号是"软中断"：pending 位向量 + blocked 位向量
  - 信号处理是异步的，不要在信号处理函数中调用非重入函数
- **习题**：8.1-8.10（进程）、8.11-8.20（信号）、8.21-8.26（setjmp）
- **视频**：CMU 15-213 Lecture 14~15 (Exceptional Control Flow)

---

### 第 5 周：虚拟内存

#### Ch9 - 虚拟内存

- **阅读**：8-10h
- **内容**：地址空间、页表、地址翻译、TLB、多级页表、mmap、动态内存分配
- **要点**：
  - **核心思路**：虚拟内存是磁盘的缓存（而不是内存的缓存）
  - 页表项：valid + dirty + accessed + permissions + PPN
  - **TLB 是关键**：TLB miss 后要走 4 级页表（4 次内存访问！）
  - 多级页表节省内存：如果一级页表的某个 entry 为空，就不用分配下级页表
  - mmap 内存映射文件：高效 I/O
- **习题**：9.1-9.8（地址翻译）、9.9-9.15（页表）、9.16-9.20（TLB）
- **视频**：CMU 15-213 Lecture 17~18 (Virtual Memory Concepts & Systems)

#### 🧪 Lab（可选）：Shell Lab

如果进度快，可以提前进入；否则跳过，后面做 Malloc Lab。

- **时间**：10-15h
- **任务**：实现一个支持 job control 的 Unix shell（类似 tsh）
- **核心技能**：fork/execve/waitpid、信号处理、作业控制
- **关键技巧**：
  - `eval` 函数：解析命令行并在子进程执行
  - `builtin_cmd`：处理 quit/fg/bg/jobs 内建命令
  - `sigchld_handler`：回收子进程，避免僵尸
  - `sigint_handler` / `sigtstp_handler`：处理 Ctrl-C/Ctrl-Z
  - **坑**：竞争条件（race condition）—— fork 和信号处理之间有窗口期
  - 用 `sigprocmask` 阻塞/解除阻塞信号

---

### 第 6 周：动态内存分配（接 Ch9）

- **阅读**：Ch9 第 9 节（动态内存分配）再精读，4-6h
- **内容**：隐式空闲链表、显式空闲链表、分离适配（segregated fit）、buddy system
- **要点**：
  - 隐式链表：每个块有 header（大小 + allocated 位）
  - **最小块大小**：16 字节（4 header + 4 footer + 8 最小载荷）
  - 放置策略：first fit / next fit / best fit
  - 碎片：内部碎片（padding）vs 外部碎片（无法合并的有空闲区域）
  - 重排策略：`sbrk` 或 `mmap` 扩展堆
- **习题**：9.21-9.28（分配器设计）

#### 🧪 Lab：Malloc Lab ⭐⭐⭐⭐

- **时间**：15-20h（CSAPP 最难 Lab）
- **任务**：自己实现 `malloc`、`free`、`realloc`
- **核心技能**：指针操作、内存布局设计、性能调试
- **评测指标**：吞吐量（速度）和利用率（空间）的加权评分
- **推荐实现路径**：

  ```
  版本 1（必做）：隐式空闲链表 + first fit  → 理解基础
  版本 2（推荐）：显式空闲链表 + LIFO     → 显著提升
  版本 3（挑战）：分离适配 segregated fit  → 冲击满分
  ```

- **关键技巧**：
  - **重要宏定义**：
    ```c
    #define WSIZE 4
    #define DSIZE 8
    #define CHUNKSIZE (1 << 12)  // 扩展堆时使用
    #define MAX(x, y) ((x) > (y) ? (x) : (y))
    #define PACK(size, alloc) ((size) | (alloc))
    #define GET(p) (*(unsigned int *)(p))
    #define PUT(p, val) (*(unsigned int *)(p) = (val))
    #define GET_SIZE(p) (GET(p) & ~0x7)
    #define GET_ALLOC(p) (GET(p) & 0x1)
    #define HDRP(bp) ((char *)(bp) - WSIZE)
    #define FTRP(bp) ((char *)(bp) + GET_SIZE(HDRP(bp)) - DSIZE)
    #define NEXT_BLKP(bp) ((char *)(bp) + GET_SIZE((char *)(bp) - WSIZE))
    #define PREV_BLKP(bp) ((char *)(bp) - GET_SIZE((char *)(bp) - DSIZE))
    ```
  - **边界标记 coalescing**：利用 header + footer 实现 O(1) 合并
  - **分离空闲链表**：每个 size class 一个桶，快速查找
  - **调试技巧**：用 `mm_check` 检查堆一致性
  - **性能平衡**：不要一味追求空间利用率（会拖慢分配速度）

---

### 第 7 周：系统 I/O + 网络编程

#### Ch10 - 系统级 I/O

- **阅读**：2-3h
- **内容**：Unix I/O、RIO 包、文件元数据、目录操作、重定向
- **要点**：
  - **文件描述符表**：每个进程一张表，fork 时会复制
  - **重定向**：`dup2(fd1, fd2)` 使 fd2 指向 fd1 的文件
  - 标准 I/O 函数（如 `printf`）与 Unix I/O（如 `write`）的区别
  - RIO（Robust I/O）包处理信号中断场景
- **习题**：10.1-10.8

#### Ch11 - 网络编程

- **阅读**：4-6h
- **内容**：TCP/IP 协议栈、socket 抽象、C/S 模型、HTTP 协议
- **要点**：
  - **socket 本质**：也是一种文件描述符（一切皆文件）
  - 服务端：`socket → bind → listen → accept → 处理 → close`
  - 客户端：`socket → connect → 处理 → close`
  - `hostent` 结构：DNS 解析
  - **字节序转换**：`htonl`/`htons`/`ntohl`/`ntohs`
- **习题**：11.1-11.6
- **视频**：CMU 15-213 Lecture 21~22 (Network Programming)

---

### 第 8 周：并发编程 + 复习

#### Ch12 - 并发编程

- **阅读**：5-7h
- **内容**：线程 vs 进程、POSIX 线程、互斥锁、信号量、生产者-消费者模式、读者-写者问题
- **要点**：
  - **共享变量**：全局变量 = 所有线程共享；局部变量 = 每个线程独享
  - **竞争条件**：多个线程同时读写同一变量
  - 互斥锁 `pthread_mutex_t`：保护临界区
  - 信号量 `sem_t`：计数信号量，可用于生产者和消费者同步
  - **死锁**：多个线程互相等待对方持有的锁
  - 线程安全函数 = 可重入函数 + 同步保护
- **习题**：12.1-12.10（基础）、12.11-12.20（进阶）
- **视频**：CMU 15-213 Lecture 23~25 (Concurrent Programming & Synchronization)

#### 🧪 Lab：Proxy Lab

- **时间**：15-20h
- **任务**：实现一个支持并发的 HTTP 代理服务器
- **核心技能**：socket 编程、HTTP 协议解析、并发设计、LRU 缓存
- **推荐实现路径**：

  ```
  Part 1：单线程代理（顺序处理请求）
  Part 2：多线程代理（每个请求一个线程）
  Part 3（可选扩展）：添加 LRU 缓存
  ```

- **关键技巧**：
  - 注意解析 HTTP 请求的头部（Host, User-Agent 等）
  - `open_clientfd` 连接目标服务器
  - 内容缓存用 LRU：双向链表 + 哈希表
  - **坑**：处理 HTTP 持久连接（Connection: keep-alive）
  - **坑**：正确转发二进制内容（不能用字符串函数处理！）
  - 最后的验证用浏览器设置代理到你的服务，能正常浏览网页

---

## 7. Lab 详解

### 7.1 Lab 一览表

| Lab | 对应章节 | 难度 | 建议时间 | 核心技能 |
|-----|---------|------|----------|----------|
| Data Lab | Ch2 | ⭐⭐⭐ | 8-15h | 位运算、编码理解 |
| Bomb Lab | Ch3(1-7) | ⭐⭐ | 6-10h | GDB、汇编阅读 |
| Attack Lab | Ch3(8-10) | ⭐⭐⭐ | 8-12h | 逆向、ROP |
| **Architecture Lab** | **Ch4** | ⭐⭐⭐ | **10-15h** | **Y86-64、流水线处理器** |
| Cache Lab | Ch6 | ⭐⭐⭐ | 10-15h | 缓存模拟与优化 |
| Shell Lab | Ch8 | ⭐⭐⭐ | 10-15h | 进程控制、信号 |
| Malloc Lab | Ch9 | ⭐⭐⭐⭐ | 15-20h | 内存管理 |
| Proxy Lab | Ch11-12 | ⭐⭐⭐⭐ | 15-20h | 网络并发编程 |

### 7.2 完成顺序的建议

```
Data Lab → Bomb Lab → Attack Lab → Architecture Lab → Cache Lab → Malloc Lab → Proxy Lab
                                    ↓
                              Shell Lab（选做，可被 OS 课程替代）
```

**如果时间不够**：

- **保底**：Data Lab + Bomb Lab + Cache Lab + Malloc Lab
- **推荐**：上面五个 + Attack Lab + Architecture Lab + Proxy Lab
- **完美**：全部 Lab 做完

### 7.3 调试 Lab 的工具

| Lab | 推荐工具 | 使用方式 |
|-----|----------|----------|
| Data Lab | `dlc`(位运算检查器)、`btest`(自动化测试) | `make` 后运行 |
| Bomb Lab | **GDB** | `gdb bomb` |
| Attack Lab | `hex2raw`、GDB | 构造输入串测试 |
| Architecture Lab | `seq`、`pipe`(Y86-64 模拟器) | `./seq -g xxx.yo` 可视化调试 |
| Cache Lab | `csim-ref`(参考实现)、Python 脚本可视化 | `make` + `python test-csim` |
| Shell Lab | GDB + `tshref`(参考 shell) | 对比行为 |
| Malloc Lab | `mdriver`(评分)、Valgrind | `./mdriver -t ./traces/` |
| Proxy Lab | `nop-server.py`, `free-port.sh`, `tiny` | `driver.sh` 自动化评分 |

### 7.4 通用调试技巧

```bash
# 开启 AddressSanitizer（检测内存错误）
gcc -Og -g -fsanitize=address -o prog prog.c

# Valgrind 检测（Malloc Lab 必用）
valgrind --leak-check=full ./prog

# GDB 常用命令速记
gdb prog
(gdb) r          # 运行
(gdb) b 函数名   # 下断点
(gdb) b *0x地址  # 在地址下断点
(gdb) n          # next 单步（不进入函数）
(gdb) s          # step 单步（进入函数）
(gdb) si         # step instruction 单条指令
(gdb) p 变量     # 打印变量
(gdb) x/10gx $rsp  # 以 8 字节为单位显示栈上 10 个值
(gdb) info frame # 显示当前栈帧
(gdb) bt         # 回溯栈
(gdb) layout asm # 显示汇编窗口
```

---

## 8. 每日建议节奏

### 正常节奏（周一至周五）

```
上午 09:00-11:00  读教材 1 章 + 做习题
下午 14:00-15:30  看对应 CMU 15-213 视频（1.3x-1.5x）
下午 15:30-17:30  Lab 实战
晚上 20:00-22:00  Lab 继续 / 整理笔记
```

### 周末节奏（周六或周日）

```
全天集中攻坚 Lab（6-8h）
```

### 卡住时的处理

1. **重读对应章节**（15min）
2. **看 CMU 视频对应部分**（30min）
3. **查官方 hint / FAQ**（15min）
4. **还是不行？** → 放下，第二天再看；有时"睡一觉就知道答案了"
5. **讨论**：找也在学 CSAPP 的同学交流

### 防摆烂建议

- 每天设置一个"最小完成量"：比如至少读完 5 页 + 做 1 个 Lab 函数
- 用番茄钟：25min 专注 + 5min 休息
- 每周日花 30min 回顾本周进度，调整下周计划

---

## 9. 给竞赛生的特别提醒

你是信息学竞赛背景，有些方面是优势，有些方面是"思维陷阱"：

### ✅ 你的优势

| 方面 | 说明 |
|------|------|
| **位运算** | Ch2 非常轻松，Data Lab 可能半天搞定 |
| **C 语言基础** | 语法层面无压力，熟悉指针 |
| **抽象能力** | 能够快速理解层次化体系结构 |
| **调试能力** | 竞赛 debug 经验可迁移到 GDB |

### ⚠️ 你的陷阱

| 陷阱 | 说明 |
|------|------|
| **"竞赛写法"vs"工程写法"** | OI 里不在意内存布局、cache 友好性、健壮性；CSAPP 要求你关注这些 |
| **忽视局部性** | OI 选手写出 O(n²) 算法然后感叹常数大——往往是 cache miss 导致的！ |
| **对底层不感兴趣** | CSAPP 的精髓在于"为什么这样设计"，不是"能用来干什么" |
| **跳读** | 竞赛解答可以跳读，CSAPP 有些章节看似简单但 Lab 会考细节 |

### 💡 具体建议

1. **Ch2 不要 skip 习题**：也许你觉得位运算太简单了，但 Data Lab 对编码规范的理解要求很高
2. **Ch3 从零学 AT&T 语法**：不要依赖 Intel 语法的经验，两者操作数顺序相反，越混淆越危险
3. **不要跳过 Ch6**：在 OI 里你可能从没考虑过缓存，但这是现代性能瓶颈的核心
4. **Malloc Lab 值得花 20h**：这可能是你第一次写"真正的"工程代码（健壮性、性能、资源管理）
5. **不要抄网上的 Lab 答案**：CSAPP 的 Lab 设计得非常精良，自己探索得到的理解远超直接看答案

---

## 10. 检查清单

### 📋 每日检查

- [ ] 今天读了 __ 页书，做了 __ 道习题
- [ ] Lab 进度：__ 个函数通过 / 正在调试 __
- [ ] 遇到了什么问题：__________________
- [ ] 明天计划：__________________

### 📋 每周总结

#### W1（Ch1-Ch2 + Data Lab）
- [ ] 理解补码编码
- [ ] 理解 IEEE 754 浮点格式
- [ ] Data Lab 全部通过（btest + dlc）
- [ ] 做完 Ch2 习题

#### W2（Ch3 1-7 + Bomb Lab）
- [ ] 能用 AT&T 语法阅读汇编
- [ ] 理解栈帧布局
- [ ] 掌握 GDB 基本操作
- [ ] Bomb Lab 通过全部 6 个 phase

#### W3（Ch3 8-10 + Ch5 + Ch6 + Attack Lab + Cache Lab）
- [ ] 理解缓冲区溢出原理
- [ ] 理解缓存结构（直接映射/组相联/全相联）
- [ ] Attack Lab 全部阶段通过
- [ ] Cache Lab Part A + Part B 完成

#### W4（Ch7 + Ch8）
- [ ] 理解 ELF 文件结构和符号解析规则
- [ ] 理解 fork/execve 语义
- [ ] 能处理信号和避免竞争条件

#### W5（Ch9 1-8）
- [ ] 理解虚拟地址翻译过程（Linux 四级页表）
- [ ] 理解 TLB 的作用

#### W6（Ch9.9 + Malloc Lab）
- [ ] 理解隐式空闲链表和显式空闲链表
- [ ] Malloc Lab 评分达到 80+（或满意分数）

#### W7（Ch10 + Ch11）
- [ ] 理解文件描述符和重定向
- [ ] 理解 socket 编程模型

#### W8（Ch12 + Proxy Lab）
- [ ] 理解线程同步机制
- [ ] Proxy Lab 完成（支持并发）

---

## 📚 拓展资源

### 若 CSAPP 学完后想继续深入

| 方向 | 下一门课 | 教材 |
|------|----------|------|
| **操作系统** | MIT 6.S081 / xv6 | 《Operating Systems: Three Easy Pieces》 |
| **体系结构** | CMU 18-447 / 本科体系结构 | 《Computer Architecture: A Quantitative Approach》 |
| **编译原理** | Stanford CS143 | 《Dragon Book》（龙书） |
| **计算机网络** | Stanford CS144 | 《计算机网络：自顶向下方法》 |
| **并行计算** | UIUC CS 420 | 《Parallel Computer Architecture》 |

---

*这份计划是为你量身定制的起点，可以根据实际进度灵活调整。关键不是读完多少章，而是 Lab 做完多少个、理解有多深。*

*祝你暑假学习顺利！*
