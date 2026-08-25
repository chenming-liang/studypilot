# getopt：解析命令行参数（Cache Lab Part A）

> **用途**：Cache Lab Part A 的 `csim.c` 需要用 `getopt` 解析 `-s <s> -E <E> -b <b> -t <tracefile>` 这类命令行参数。
> **环境**：Linux / WSL。`getopt` 声明在 `<unistd.h>`。

---

## 1. 目标长什么样

参考模拟器这样被调用：

```bash
./csim-ref -s 4 -E 1 -b 4 -t traces/yi.trace
./csim-ref -s 2 -E 4 -b 3 -v -t traces/yi.trace
```

程序要从命令行拿到三个数字（`s`、`E`、`b`）和一个 trace 文件路径，还要支持可选的 `-v`（verbose 模式）。这就是 `getopt` 的活。

## 2. getopt 是什么

`getopt` 是标准库（`<unistd.h>`）提供的命令行解析函数：它**一次只解析一个选项**，你用 `while` 循环把它榨干。

```c
#include <unistd.h>
int getopt(int argc, char *const argv[], const char *optstring);
```

三个要点：

- **返回值**：每次调用返回**一个选项字符**（如 `'s'`）；全部解析完后返回 **`-1`**，循环结束。
- **`optarg`**：如果当前选项**需要参数**，参数就放在 `optarg`（`char *`，记得转成数字）。
- **`optind`**：下一个待解析参数的下标；解析结束后 `argv[optind]` 起是**非选项参数**（Cache Lab 用不到）。

### optstring（选项串）怎么写

```c
"hvs:E:b:t:"
```

- 直接写选项字符：`h`、`v` 是**无参数**选项；
- 字符后加 **`:`**：该选项**必须有参数**——`s:`、`E:`、`b:`、`t:`；
- 参数值存在 `optarg`（字符串），用 `atoi`/`strtol` 转成整数。

## 3. 标准骨架（while + switch）

```c
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    int s = 0, E = 0, b = 0;
    int verbose = 0;
    char *tracefile = NULL;
    int opt;

    while ((opt = getopt(argc, argv, "hvs:E:b:t:")) != -1) {
        switch (opt) {
        case 's': s = atoi(optarg); break;
        case 'E': E = atoi(optarg); break;
        case 'b': b = atoi(optarg); break;
        case 't': tracefile = optarg; break;
        case 'v': verbose = 1; break;
        case 'h':
        default:
            printf("用法: %s -s <s> -E <E> -b <b> -t <tracefile> [-v]\n", argv[0]);
            exit(0);
        }
    }
    /* 到这里，s / E / b / tracefile 都拿到手了 */
    return 0;
}
```

## 4. 逐行解读

- `while ((opt = getopt(...)) != -1)`：循环，直到没有选项可解析。
- `case 's': s = atoi(optarg);`：遇到 `-s` 时，`getopt` 把后面的 `"4"` 放进 `optarg`，`atoi` 把它变成整数 4。
- `case 'h':` 与 `default:`：处理帮助；`default` 还会兜住"**未知选项**"（`getopt` 返回 `?`）。
- **参数顺序无关**：`-s 4 -E 1 -b 4 -t file` 和 `-t file -b 4 -E 1 -s 4` 等价；也支持 `-s4` 连写、`-s 4` 分开写。

## 5. 错误与健壮性

**`atoi` 还是 `strtol`？** `atoi("4")=4` 但 `atoi("abc")=0`（不报错）——Cache Lab 用 `atoi` 就够了；想发现非法输入才用 `strtol` + `errno`，这里不必讲究。

**参数缺了怎么办？** 先设无效初值，再检查有没有被赋值：

```c
int s = -1, E = -1, b = -1;      /* -1 表示"还没给" */
char *tracefile = NULL;
...
case 's': s = atoi(optarg); break;
...
if (s < 0 || E < 0 || b < 0 || tracefile == NULL) {
    fprintf(stderr, "缺少必要参数！\n");
    exit(1);
}
```

**带 `:` 的选项却没给参数**：`getopt` 返回 `?`、`optarg` 为 `NULL`——落在 `default` 分支打印用法即可。

## 6. 完整可跑的示例（csim.c 版）

```c
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    int s = -1, E = -1, b = -1;          /* -1 = 还没给 */
    int verbose = 0;
    char *tracefile = NULL;
    int opt;

    while ((opt = getopt(argc, argv, "hvs:E:b:t:")) != -1) {
        switch (opt) {
        case 's': s = atoi(optarg); break;
        case 'E': E = atoi(optarg); break;
        case 'b': b = atoi(optarg); break;
        case 't': tracefile = optarg; break;
        case 'v': verbose = 1; break;
        case 'h':
        default:
            fprintf(stderr, "用法: %s -s <setbits> -E <lines> -b <blockbits> -t <tracefile> [-v]\n", argv[0]);
            exit(0);
        }
    }

    if (s < 0 || E < 0 || b < 0 || tracefile == NULL) {
        fprintf(stderr, "缺少参数！用法: %s -s <setbits> -E <lines> -b <blockbits> -t <tracefile>\n", argv[0]);
        exit(1);
    }

    printf("s=%d, E=%d, b=%d, trace=%s, verbose=%d\n", s, E, b, tracefile, verbose);
    return 0;
}
```

## 7. 常见坑

- **`optstring` 忘写 `:`**：`-s 4` 会被当成"`-s` 无参数"，`4` 变成下一个"选项"，解析就乱了。**带参数的选项必须在字符后加 `:`**。
- **`atoi(optarg)` 别写成 `atoi(&optarg)`**：`optarg` 已经是 `char *`，直接传。
- **`default` 要兜 `?`**：未知选项或缺参数时 `getopt` 返回 `?`，`default` 负责打印用法。
- **`-h` 用 `exit(0)`、参数缺失用 `exit(1)`**：前者是"正常帮用户看用法"，后者是"出错了"。
- **别忘了 `#include <unistd.h>`**（`atoi` 还要 `<stdlib.h>`）。
