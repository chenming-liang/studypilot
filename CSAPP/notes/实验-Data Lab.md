# Data Lab (Lab 1) 解答分析

> 原始代码见 `../bits.c`，此文件收录有显著改进的解法。

---

## 整数部分

### 1. bitXor — 无异样

你的解法（仅用 `~`、`&` 实现异或）是标准答案，8 ops，已经最优。

---

### 2. tmin — 无异样

`1 << 31`，1 op，最优。

---

### 3. isTmax

**你的解法**：8 ops。先检查 `~x == x+1`（Tmax 和 -1 都满足），再用 `!!(~x)` 排除 `x = -1`。

**更简洁的写法**（6 ops）：

```c
int isTmax(int x) {
    int not_x = ~x;          // Tmax → 0x80000000, -1 → 0
    return !(not_x ^ (x + 1)) & !!not_x;
}
```

思路不变，但用变量 `not_x` 把 `~x` 的结果复用了两次，减少计算量。

---

### 4. allOddBits — 无异样

标准做法：构造 `0xAAAAAAAA` 掩码，与、判等。8 ops，最优。

---

### 5. negate — 无异样

`~x + 1`，最优。

---

### 6. isAsciiDigit — 无异样

用 `x - 0x30 >= 0` 和 `0x3a - x >= 0` 的符号位判断，10 ops，简洁正确。

---

### 7. conditional

**你的解法**：9 ops。将 `x` 转为 `bool`（0 或 1），再扩展为全 0 / 全 1 掩码，分别与 `y`、`z` 做与运算再相加。

**更简洁的写法**（7 ops）：

```c
int conditional(int x, int y, int z) {
    int mask = (!x) + (~0);   // x == 0 → mask = 0; x != 0 → mask = -1
    return (mask & y) | (~mask & z);
}
```

**原理**：`!x` 返回 1（x=0）或 0（x≠0）。加上 `~0`（即 -1）：
- 若 x ≠ 0：`0 + (-1) = -1` → `mask & y = y`，`~mask & z = 0`
- 若 x = 0：`1 + (-1) = 0` → `mask & y = 0`，`~mask & z = z`

比你的解法少了两次取反和一次加法。

---

### 8. isLessOrEqual

**你的解法**：思路正确，但分支逻辑较复杂，不易验证。

**更清晰的写法**（13 ops）：

```c
int isLessOrEqual(int x, int y) {
    int sign_x = x >> 31;
    int sign_y = y >> 31;
    int same_sign = !(sign_x ^ sign_y);    // 符号相同？
    int diff = y + (~x + 1);               // y - x
    // 同号：看 diff >= 0，异号：看 x < 0（即 x 是负数则 x <= y 成立）
    return (same_sign & !(diff >> 31)) | (!same_sign & sign_x);
}
```

**原理**：

- **异号情形**（x 负 y 正）：`x <= y` 恒真，只需判断 `x < 0`（即 `sign_x` 为 -1）。
- **同号情形**：`y - x` 不会溢出（因为同号），检查差的符号位即可。

这个版本把两种情形显式分开，逻辑一目了然。

---

### 9. logicalNeg — 无异样

利用 `x` 与 `-x` 的符号位关系：若 x ≠ 0，则 `x | -x` 的符号位为 1。5 ops，最优。

---

### 10. howManyBits

**你的解法**：二分查找实现正确，但条件加法的写法 (`(~(!(!y1)) + 1) & 16`) 可读性差。

**更清晰的写法**（统一用 `!!` + 移位模式）：

```c
int howManyBits(int x) {
    int y = x ^ (x >> 1);     // 归一化：正数找最高位 1，负数找最高位 0
    int cnt = !!y;            // y = 0 说明 x = 0 或 -1，只需 1 bit
    
    int bit16 = !!(y >> 16);
    cnt += bit16 << 4;
    y >>= bit16 << 4;
    
    int bit8 = !!(y >> 8);
    cnt += bit8 << 3;
    y >>= bit8 << 3;
    
    int bit4 = !!(y >> 4);
    cnt += bit4 << 2;
    y >>= bit4 << 2;
    
    int bit2 = !!(y >> 2);
    cnt += bit2 << 1;
    y >>= bit2 << 1;
    
    int bit1 = !!(y >> 1);
    cnt += bit1;
    
    return cnt + 1;
}
```

**原理**：每轮用 `!!(y >> k)` 判断高位是否有有效数据，若有则把计数加上 k 并将 y 右移 k 位（缩小搜索范围），若无则计数不变、y 也不动。比用掩码条件加法的写法直观得多。操作数约 35 个，远低于上限 90。

---

## 浮点部分

### 11. floatScale2

**你的解法**：逻辑正确，但判断条件写为 `(1u << 23) - 1` 等重复运算，可读性有提升空间。

**更清晰的写法**：

```c
unsigned floatScale2(unsigned uf) {
    unsigned exp = (uf >> 23) & 0xFF;
    unsigned frac = uf & 0x7FFFFF;
    unsigned s = uf & 0x80000000;
    
    if (exp == 0xFF)               // NaN 或 Inf，直接返回
        return uf;
    
    if (exp == 0) {                // 非规格化数
        frac <<= 1;
        if (frac & 0x800000) {     // 溢出到规格化范围
            frac &= 0x7FFFFF;
            exp = 1;               // 进位到 exp
        }
    } else {                       // 规格化数
        exp++;
    }
    
    return s | (exp << 23) | frac;
}
```

主要改动：用 `0xFF`、`0x7FFFFF`、`0x80000000` 等一目了然的常量，替代 `(1u << 8) - 1`、`((1u << 23) - 1)` 等计算。

---

### 12. floatFloat2Int

**你的解法中存在一个未定义行为（UB）bug**：

```c
if(E < 0)x = 0, y >>= E;
```

当 `E < 0` 时，`y >>= (-E)` 中的移位位数为负数——**C 标准规定移位位数为负数属于未定义行为**。虽然本意是当浮点数小于 1 时返回 0，但这里走了 UB 路径。

**改正后的版本**：

```c
int floatFloat2Int(unsigned uf) {
    int exp = (uf >> 23) & 0xFF;
    int frac = uf & 0x7FFFFF;
    int s = uf >> 31;
    int E = exp - 127;
    
    // NaN 或 Inf → 返回 0x80000000
    if (exp == 0xFF)
        return 0x80000000u;
    // 0 或 |value| < 1 → 返回 0
    if (exp == 0 || E < 0)
        return 0;
    // 溢出（E > 30 时 int 无法表示）
    if (E >= 31)
        return 0x80000000u;
    
    // 构造整数：隐含的 1 + frac
    int val = (1 << 23) | frac;
    
    if (E > 23)
        val <<= (E - 23);       // 指数大，左移
    else
        val >>= (23 - E);        // 指数小，右移（截断舍入）
    
    return s ? -val : val;
}
```

> 注意：C 的 float→int 转换采用**向 0 截断**（truncation），不是舍入到最近偶数。所以不需要舍入逻辑，直接右移丢弃小数位即可。

---

### 13. floatPower2

**你的解法**：基本正确，但 `x += 126` 的语义不直观。

**更清晰的写法**：

```c
unsigned floatPower2(int x) {
    // 2^x = (-1)^0 * 1.0 * 2^x → exp = x + 127
    int exp = x + 127;
    
    if (exp >= 0xFF)              // 太大 → +inf
        return 0x7F800000;        // exp = 255, frac = 0
    else if (exp <= 0) {          // 太小 → 非规格化数或 0
        // 2^x 的非规格化表示，移位量 = (1 - bias) - x = 126 - x
        if (-x >= 23) return 0;   // 超出非规格化表示能力
        return 1u << (23 - 1 + x);// 注意 x 为负数，等价于 1 << (23 + x - 1)
    }
    
    return exp << 23;             // 规格化数，frac = 0
}
```

> 注意：`floatPower2(0)` 应返回 `1.0` 的位表示，即 `0x3F800000`（exp = 127, frac = 0）。`exp = 0 + 127 = 127`，左移 23 位得 `0x3F800000`，正确。

---

## 汇总

| 函数 | 你的 ops | 改进 ops | 备注 |
|------|---------|---------|------|
| bitXor | 8 | — | 最优 |
| tmin | 1 | — | 最优 |
| isTmax | 8 | 6 | 减少重复计算 |
| allOddBits | 8 | — | 最优 |
| negate | 2 | — | 最优 |
| isAsciiDigit | 10 | — | 最优 |
| conditional | 9 | 7 | 掩码法更简洁 |
| isLessOrEqual | 12 | 13 | 牺牲 1 op 换可读性 |
| logicalNeg | 5 | — | 最优 |
| howManyBits | ~40 | ~35 | 统一为 `!!` + 移位模式 |
| floatScale2 | 12 | 12 | 常量更清晰 |
| floatFloat2Int | 16 | 14 | 修复 UB bug |
| floatPower2 | 15 | 8 | 大幅简化 |

---

[[t大课程/CSAPP/notes/README|返回索引]]
