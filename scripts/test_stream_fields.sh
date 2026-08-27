#!/usr/bin/env bash
# M0 第③项一次性实测：DeepSeek 流式响应 + 思考模式字段结构。
# 从 config.toml 读 endpoint/api_key/model；输出 SSE delta 字段结构分析。
set -euo pipefail

CFG="$(dirname "$0")/../config.toml"
ENDPOINT=$(grep -m1 '^endpoint' "$CFG" | sed 's/.*= *"\(.*\)".*/\1/')
KEY=$(grep -m1 '^api_key' "$CFG" | sed 's/.*= *"\(.*\)".*/\1/')
MODEL=$(grep -m1 '^model' "$CFG" | sed 's/.*= *"\(.*\)".*/\1/')

[ -n "$KEY" ] || { echo "ERROR: config.toml 中 api_key 为空" >&2; exit 1; }

echo "== provider: $ENDPOINT  model: $MODEL =="

PAYLOAD=$(cat <<EOF
{
  "model": "$MODEL",
  "stream": true,
  "messages": [{"role": "user", "content": "9.11 和 9.8 哪个大？简短回答"}]
}
EOF
)

OUT=$(mktemp)
curl -sS -N "$ENDPOINT/chat/completions" \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" > "$OUT"

echo "== 原始前 5 行 =="
head -5 "$OUT"

python3 - "$OUT" <<'PY'
import json, sys
chunks = []
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line.startswith("data:"):
        continue
    data = line[5:].strip()
    if data == "[DONE]":
        break
    try:
        chunks.append(json.loads(data))
    except json.JSONDecodeError:
        pass

print(f"\n== 共 {len(chunks)} 个 chunk ==")
first, last = chunks[0], chunks[-1]

def shape(o, depth=0):
    if isinstance(o, dict):
        return {k: shape(v, depth+1) for k, v in o.items()}
    return type(o).__name__

print("\n== 首 chunk 结构 ==")
print(json.dumps(shape(first), ensure_ascii=False, indent=1))
print("\n== 首 chunk 完整内容（截断）==")
s = json.dumps(first, ensure_ascii=False)
print(s[:600])

# 统计 delta 中出现过的字段
keys = {}
reasoning_variants = set()
content_len = reasoning_len = 0
finish_reasons = set()
usage = None
for c in chunks:
    for ch in c.get("choices", []):
        d = ch.get("delta", {})
        for k in d:
            keys[k] = keys.get(k, 0) + 1
        for k in ("reasoning_content", "reasoning"):
            v = d.get(k)
            if v is not None:
                reasoning_variants.add(k)
                reasoning_len += len(v)
        content_len += len(d.get("content") or "")
        if ch.get("finish_reason"):
            finish_reasons.add(ch["finish_reason"])
    if c.get("usage"):
        usage = c["usage"]

print("\n== delta 出现过的字段及次数 ==")
print(keys)
print(f"\nreasoning 字段变体: {reasoning_variants or '无'}")
print(f"reasoning 总字符: {reasoning_len}   content 总字符: {content_len}")
print(f"finish_reason: {finish_reasons}")
if usage:
    print("usage:", json.dumps(usage, ensure_ascii=False))

# 拼出完整回答验证
text = "".join((c["choices"][0]["delta"].get("content") or "") for c in chunks if c.get("choices"))
think = "".join(
    (c["choices"][0]["delta"].get(k) or "")
    for c in chunks if c.get("choices")
    for k in ("reasoning_content", "reasoning")
)
print("\n== 思考文本前 200 字 ==")
print(think[:200])
print("\n== 最终回答 ==")
print(text[:300])
PY
rm -f "$OUT"
