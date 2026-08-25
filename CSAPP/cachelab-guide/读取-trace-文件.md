# 读取 trace 文件（Cache Lab Part A）

> 配套 `getopt-命令行参数解析.md`。拿到 `-s -E -b -t` 之后，下一步就是打开 trace 文件、逐行读、模拟缓存访问、计数。

---

## 1. trace 文件长什么样

每一行是一次**内存访问**，格式是三个字段、用空格/逗号隔开：

```
操作  地址(十六进制, 无 0x)  大小(十进制)
 L    10f1b010,1
 S    60f1b030,4
 M    20f1b020,2
 I    0400d7d4,8
```

真实文件示例：

```
 L 10,1
 M 20,1
 L 22,1
 S 18,1
 L 110,1
 L 210,1
 M 12,1
```

## 2. 四种操作的含义

| 操作 | 含义 | 你要做的 |
|---|---|---|
| `I` | 取指（instruction load） | **跳过**，不计数 |
| `L` | 数据读（load） | 做 **1 次**缓存访问 |
| `S` | 数据写（store） | 做 **1 次**缓存访问 |
| `M` | 数据修改（modify） | **先 L 后 S**，做 **2 次**缓存访问 |

关键点：

- **`I` 一律忽略**——模拟器只关心数据访问；
- **`M` 算两次**：一次 load 命中/缺失 + 一次 store 命中/缺失，可能贡献 0/1/2 次缺失、0/1/2 次驱逐。

## 3. 怎么逐行读

标准做法：`fgets` 读一行，`sscanf` 拆字段。地址用 `%x`（十六进制）、大小用 `%d`、操作符用 `%c`：

```c
char identifier;      /* L / S / M / I */
unsigned address;     /* 地址，十六进制 */
int size;             /* 大小（本 Lab 用不到，但要读掉） */
char buf[MAXLINE];
FILE *trace = fopen(tracefile, "r");

while (fgets(buf, MAXLINE, trace) != NULL) {
    sscanf(buf, " %c %x,%d", &identifier, &address, &size);
    if (identifier == 'I')
        continue;                             /* 跳过取指 */
    if (identifier == 'M')
        access(cache, s, E, b, address, &hits, &misses, &evictions);
    access(cache, s, E, b, address, &hits, &misses, &evictions);
}
```

注意 `sscanf` 格式串开头的空格 `" %c"`——它跳过行首可能存在的空白；`%x` 直接读十六进制地址。

## 4. 拿到地址后：模拟一次缓存访问

一次访问 = 拆地址 → 找组 → 比 tag → 命中/缺失/驱逐。

```c
void access(line **cache, int s, int E, int b, unsigned addr,
            int *hits, int *misses, int *evictions) {
    int tag = addr >> (s + b);                    /* 高 t 位 */
    int set = (addr >> b) & ((1 << s) - 1);       /* 中间 s 位 */

    /* ① 找组 set，看有没有 valid 且 tag 匹配的行 → 命中 */
    for (int i = 0; i < E; i++)
        if (cache[set][i].valid && cache[set][i].tag == tag) {
            (*hits)++;
            mark_used(cache, set, i);             /* LRU：标记刚被用 */
            return;
        }

    /* ② 缺失：misses++，找空行放进 */
    (*misses)++;
    for (int i = 0; i < E; i++)
        if (!cache[set][i].valid) {
            cache[set][i].valid = 1;
            cache[set][i].tag = tag;
            mark_used(cache, set, i);
            return;
        }

    /* ③ 组满：evictions++，驱逐一行（要用 LRU 才能和 csim-ref 完全一致） */
    (*evictions)++;
    int victim = find_lru(cache, set, E);         /* 最久没用的行 */
    cache[set][victim].valid = 1;
    cache[set][victim].tag = tag;
    mark_used(cache, set, victim);
}
```

缓存数据结构（S 组、每组 E 行）：

```c
typedef struct {
    int valid;
    int tag;
} line;                        /* 每组 E 行，数组 cache[set][0..E-1] */

int S = 1 << s;
line **cache = malloc(S * sizeof(line *));
for (int i = 0; i < S; i++)
    cache[i] = calloc(E, sizeof(line));   /* valid 初始 0 */
```

## 5. LRU 替换（要和 csim-ref 完全一致就必须做）

**本 Lab 必须用 LRU**——`csim-ref` 用的就是 LRU，`test-csim` 会逐项对比你的输出。最简单做法：每行存一个"上次使用序号"，`mark_used` 递增一个全局计数器记下当前序号，驱逐时选序号最小的：

```c
static int tick = 0;                  /* 全局"时钟" */
/* cache[set][i] 里加一个 int used; */

void mark_used(line **cache, int set, int i) {
    cache[set][i].used = ++tick;      /* 记下"刚在第几拍被用" */
}

int find_lru(line **cache, int set, int E) {
    int best = 0;
    for (int i = 1; i < E; i++)
        if (cache[set][i].used < cache[set][best].used)
            best = i;                 /* 序号最小 = 最久没用 */
    return best;
}
```

（直接映射 E=1 时没有选择，LRU 退化为"就那一行"。）

## 6. 写策略（影响计数）

本 Lab 模拟**写回 (write-back) + 写分配 (write-allocate)**：

- **写命中**：不 miss，直接改（本 Lab 不真存数据，只数命中）。
- **写缺失**：先取块（**算一次 miss**），再写——即 store 缺失也算 miss。
- **驱逐**：被替换的行是 valid 的 → **算一次 eviction**（不管脏不脏，本 Lab 简化处理）。

所以上面 `access()` 对 L 和 S 一视同仁地"找组比 tag"，天然就是 write-allocate + write-back 的行为。

## 7. 完整骨架（可编译跑通）

```c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define MAXLINE 1024

typedef struct { int valid, tag, used; } line;

static int tick = 0;
static void mark_used(line **c, int set, int i) { c[set][i].used = ++tick; }
static int find_lru(line **c, int set, int E) {
    int b = 0;
    for (int i = 1; i < E; i++) if (c[set][i].used < c[set][b].used) b = i;
    return b;
}

static void access(line **c, int s, int E, int b, unsigned addr,
                   int *hits, int *misses, int *evictions) {
    int tag = addr >> (s + b);
    int set = (addr >> b) & ((1 << s) - 1);
    for (int i = 0; i < E; i++)
        if (c[set][i].valid && c[set][i].tag == tag) { (*hits)++; mark_used(c, set, i); return; }
    (*misses)++;
    for (int i = 0; i < E; i++)
        if (!c[set][i].valid) { c[set][i].valid = 1; c[set][i].tag = tag; mark_used(c, set, i); return; }
    (*evictions)++;
    int v = find_lru(c, set, E);
    c[set][v].valid = 1; c[set][v].tag = tag; mark_used(c, set, v);
}

int main(int argc, char *argv[]) {
    int s = -1, E = -1, b = -1;
    char *tracefile = NULL;
    int opt;
    while ((opt = getopt(argc, argv, "s:E:b:t:")) != -1) {
        switch (opt) {
        case 's': s = atoi(optarg); break;
        case 'E': E = atoi(optarg); break;
        case 'b': b = atoi(optarg); break;
        case 't': tracefile = optarg; break;
        default: fprintf(stderr, "用法: %s -s <s> -E <E> -b <b> -t <trace>\n", argv[0]); exit(0);
        }
    }
    if (s < 0 || E < 0 || b < 0 || !tracefile) { fprintf(stderr, "缺少参数\n"); exit(1); }

    int S = 1 << s;
    line **cache = malloc(S * sizeof(line *));
    for (int i = 0; i < S; i++) cache[i] = calloc(E, sizeof(line));

    int hits = 0, misses = 0, evictions = 0;
    char id; unsigned addr; int size; char buf[MAXLINE];
    FILE *trace = fopen(tracefile, "r");
    while (fgets(buf, MAXLINE, trace) != NULL) {
        sscanf(buf, " %c %x,%d", &id, &addr, &size);
        if (id == 'I') continue;
        if (id == 'M') access(cache, s, E, b, addr, &hits, &misses, &evictions);
        access(cache, s, E, b, addr, &hits, &misses, &evictions);
    }
    fclose(trace);

    printf("hits:%d misses:%d evictions:%d\n", hits, misses, evictions);
    return 0;
}
```

跑法（先测小样例和 `csim-ref` 对拍）：

```bash
gcc -o csim csim.c -Wall
./csim -s 4 -E 1 -b 4 -t traces/yi.trace
./csim-ref -s 4 -E 1 -b 4 -t traces/yi.trace   # 输出应完全一致
```

## 8. 常见坑

- **`M` 只调一次 access**：`M` 是两次访问，漏掉一次会差一截。
- **`I` 没跳过**：取指行会污染计数。
- **驱逐只算"组满且要放"那次**：命中不驱逐；有 valid 空行不驱逐。
- **`1 << s` 当 s 较大时**：本 Lab 的 s 都不大，够用；想严谨用 `1u << s`。
- **地址位宽**：用 `unsigned`（32 位）即可；tag 拆法是 `addr >> (s+b)`，set 是 `(addr >> b) & mask`。
