# malloc / calloc / free（动态内存分配）

> 给 OI 出身、只写过全局静态数组的你：`malloc`/`calloc` 解决"数组大小运行时才知道"的问题——`n` 是读进来才知道的，编译期开不了。

---

## 1. malloc：按字节分配

```c
void *malloc(size_t size);   // 分配 size 字节，返回首地址（void*）
```

基本用法——分配 `n` 个 `int`：

```c
int n;
scanf("%d", &n);
int *a = malloc(n * sizeof(int));   // 要 n 个 int → n × sizeof(int) 字节
if (a == NULL) { perror("malloc"); exit(1); }   // 失败返回 NULL
for (int i = 0; i < n; i++) a[i] = i;
free(a);                            // 用完必须释放
```

三个要点：

- **`sizeof(int)` 别省**：`malloc(n * 4)` 在 `int` 是 4 字节的机器上碰巧对，但不严谨；写 `sizeof(类型)` 才可移植；
- **返回 `void*`**：C 里可直接赋给 `int*`（C++ 需强转 `(int *)malloc(...)`）；
- **可能返回 NULL**：内存不够时返回 NULL，严谨代码要检查。

## 2. calloc：分配 + 清零

```c
void *calloc(size_t nmemb, size_t size);   // 分配 nmemb × size 字节，全部置 0
```

区别就一个：**`calloc` 把内存初始化为 0**，`malloc` 里面是垃圾值：

```c
int *a = calloc(n, sizeof(int));   // a[0..n-1] 全为 0
int *b = malloc(n * sizeof(int));  // b[i] 是随机垃圾值！
```

什么时候用 calloc：你本来就打算先清零（计数器数组、Cache Lab 的 `valid=0` 初始化）。用 `calloc` 省掉自己写循环清零。

## 3. free：配对的另一半

```c
void free(void *ptr);   // 把这块内存还给堆
```

铁律：

- 每个 `malloc`/`calloc` **配一个 `free`**；
- **别 `free` 两次**（`free(x); free(x);` 是未定义行为）；
- **`free` 之后别再用**（use-after-free）；
- 忘了 free = **内存泄漏**（长期运行越吃越多——第 9 章专门讲过）。

## 4. realloc：改大小

```c
void *realloc(void *ptr, size_t new_size);   // 改成 new_size 字节
```

- 可能原地扩大、也可能搬到新地址（**用返回值**）；
- 失败返回 NULL，原块不动。

```c
int *a = malloc(10 * sizeof(int));
a = realloc(a, 20 * sizeof(int));   // 变 20 个（注意用返回值）
```

## 5. 二维数组怎么分配（Cache Lab 的缓存就是二维的！）

缓存是 `cache[S 组][E 行]`，S 运行时才定，要动态分配。先分配"行指针数组"，再逐行分配：

```c
int **cache = malloc(S * sizeof(int *));        // S 个指针（指向每行）
for (int i = 0; i < S; i++)
    cache[i] = calloc(E, sizeof(line));         // 每行 E 个 line，且清零

/* 用的时候 cache[set][line] 跟静态数组一模一样 */

/* 释放：先每行、再整体（顺序反过来） */
for (int i = 0; i < S; i++) free(cache[i]);
free(cache);
```

> `cache[i] = calloc(E, sizeof(line))` 用 `calloc` 正好把每行的 `valid` 初始化成 0——trace 指南里就这么写的。

## 6. 和 CSAPP 第 9 章的关系

`malloc`/`calloc`/`free` 是**系统（C 库）给你的现成接口**。CSAPP 第 9 章「动态内存分配」（笔记 `第9章-动态内存分配`）讲的是**背后怎么实现**——`malloc` 内部靠 `sbrk`/`mmap` 要内存、用空闲链表管理块、合并碎片。你的 Lab 是"**用** malloc"；Malloc Lab 是"**自己写**一个 malloc"。

## 7. 常见坑速查

| 坑 | 错误写法 | 正确 |
|---|---|---|
| 忘 `sizeof` | `malloc(n)` | `malloc(n * sizeof(int))` |
| 忘检查 NULL | 直接用 | `if (!p) exit(1);` |
| 忘 free | 循环里 `malloc` 不释放 | 配对 `free` |
| 双 free | `free(p); free(p);` | 只一次，释放后置 NULL |
| use-after-free | `free(p); ... p[0]=1;` | 释放后不再碰 |
| 数组越界 | `a[n]`（只开了 n 个） | 下标 0..n-1 |
