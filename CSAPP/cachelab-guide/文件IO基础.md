# C 文件 I/O 基础（fopen / fscanf / fprintf）

> 给 OI 出身、只写过 `scanf`/`printf`/`stdin`/`stdout` 的你：文件 I/O 其实就一句话——**多一个 `FILE*` 参数**。你会的那些都在，只是目标从"键盘/屏幕"换成了"文件"。

---

## 1. 核心差别

OI 里你写：

```c
int n; scanf("%d", &n);    // 从标准输入读
printf("%d\n", n);         // 写到标准输出
```

文件 I/O 只是把这些的"输入/输出"换成文件：先 `fopen` 打开文件拿到**文件指针 `FILE*`**，再把它作为第一个参数传给 `fscanf`/`fprintf`：

```c
FILE *fp = fopen("input.txt", "r");   // 打开文件
int n; fscanf(fp, "%d", &n);          // 从文件读（和 scanf 一样，多个 fp）
fprintf(fp, "%d\n", n);               // 写到文件
fclose(fp);                           // 用完关闭
```

**格式串、`%d`/`%s`/`%x` 你全会**——唯一的新东西是"先 `fopen` 拿 `fp`，读写时把 `fp` 放第一个参数"。

## 2. fopen：打开文件

```c
FILE *fopen(const char *filename, const char *mode);
```

| mode | 含义 |
|---|---|
| `"r"` | 只读（文件必须存在） |
| `"w"` | 只写（不存在则建；**存在则清空**） |
| `"a"` | 追加（写，位置在末尾） |
| `"r+"` | 读写（必须存在） |
| `"w+"` | 读写（清空/创建） |
| `"a+"` | 读 + 追加写 |

**返回 NULL = 失败**（文件不存在、无权限），必须检查：

```c
FILE *fp = fopen("yi.trace", "r");
if (fp == NULL) { perror("yi.trace"); exit(1); }
```

## 3. fscanf / fprintf：和 scanf/printf 一样，多个 FILE*

```c
int fscanf(FILE *fp, const char *format, ...);   // 从 fp 读
int fprintf(FILE *fp, const char *format, ...);  // 写到 fp
```

- 格式串、转换符、指针参数，跟你熟的一模一样；
- **`fscanf` 返回成功匹配的项数**：`fscanf(fp, "%d %d", &a, &b) == 2` 表示读到了两个；返回 `EOF` 表示到文件末尾/出错。
- Cache Lab 读 trace 就这样：

```c
char id; unsigned addr; int size;
while (fscanf(fp, " %c %x,%d", &id, &addr, &size) != EOF) {
    /* 处理一次访问 */
}
```

> 也可以 `fgets` + `sscanf`（见 trace 指南）——两种都行，`fscanf` 更短。

## 4. fgets / fputs：整行

```c
char *fgets(char *s, int size, FILE *fp);   // 读一行（最多 size-1 字符，含换行）
int   fputs(const char *s, FILE *fp);       // 写一行（不带换行）
```

- `fgets` 读到换行符或 size-1 个字符为止，**会把 `\n` 存进去**；
- 返回 NULL = 读到文件末尾/出错 → 循环终止：

```c
char buf[1024];
while (fgets(buf, sizeof buf, fp) != NULL) {
    /* 处理这一行，注意 buf 末尾带个 '\n' */
}
```

## 5. fgetc / fputc：单字符

```c
int fgetc(FILE *fp);   // 读一个字符；到末尾返回 EOF
int fputc(int c, FILE *fp);
```

## 6. fread / fwrite：原始字节（二进制文件）

字符串 I/O 处理不了含 `\0` 的二进制数据，要按块读原始字节：

```c
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *fp);
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *fp);
// 返回实际读/写的"块数"；size 是每块字节数，nmemb 是块数
```

## 7. fclose 和为什么必须关

```c
int fclose(FILE *fp);   // 刷新缓冲区 + 释放
```

- 写文件是**缓冲**的：数据先攒在内存缓冲区，`fclose`/`fflush` 才真正落盘。**不 `fclose` 可能丢数据**；
- 每次 `fopen` 配一次 `fclose`（和你熟知的 `malloc`/`free` 配对直觉一样）。

## 8. 判断末尾/出错

- **`feof(fp)`**：返回非零表示**已经**读到过末尾——注意先读、读到 EOF 后它才为真，别拿它当循环条件（会多读一次）；
- **`ferror(fp)`**：出错标志；
- 最常用的写法还是 `while (fscanf(...) != EOF)` 或 `while (fgets(...) != NULL)`，天然处理末尾。

## 9. stdin / stdout / stderr 也是 FILE*

```c
fprintf(stdout, "Hello\n");    // 就是 printf
fscanf(stdin, "%d", &n);       // 就是 scanf
fprintf(stderr, "错误！\n");    // 打到错误流
```

所以 `printf` 其实是 `fprintf(stdout, ...)` 的简写。

## 10. OI → 文件 I/O 速查

| 你熟的 | 文件版 |
|---|---|
| `scanf(...)` | `fscanf(fp, ...)` |
| `printf(...)` | `fprintf(fp, ...)` |
| `getchar()` | `fgetc(fp)` |
| `gets(s)`（别再用了） | `fgets(s, size, fp)` |
| `puts(s)` | `fputs(s, fp)` 或 `fprintf(fp, "%s\n", s)` |
| 标准输入/输出 | 先 `FILE *fp = fopen(...)` |

## 11. 完整示例：读数字文件、写结果文件

```c
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    FILE *in  = fopen("in.txt",  "r");
    FILE *out = fopen("out.txt", "w");
    if (in == NULL || out == NULL) { perror("open"); exit(1); }

    int sum = 0, x;
    while (fscanf(in, "%d", &x) == 1)   /* 读到不是数字或末尾就停 */
        sum += x;
    fprintf(out, "sum = %d\n", sum);

    fclose(in);
    fclose(out);                          /* 不关可能丢数据！ */
    return 0;
}
```

## 12. 常见坑

- **fopen 没检查 NULL** → 空指针崩溃；
- **`"w"` 会清空文件**——想追加用 `"a"`；
- **写文件不 fclose** → 缓冲区的数据没落盘，丢了；
- **用 feof 判断"还有没有"** → 应该先读、看返回值；`feof` 在"读的瞬间之后"才为真；
- **fgets 读到的行带 `\n`** → 处理前想去掉：`buf[strcspn(buf, "\n")] = 0;`
- **fread/fwrite 用于二进制**——字符串函数（strlen/strcpy）别用在二进制数据上。
