#!/usr/bin/env python3
"""pdf_extract.py — pymupdf PDF 文本提取辅助脚本。

用法:
    python pdf_extract.py <file.pdf>

stdout 输出 UTF-8 JSON（Rust 侧子进程调用，须显式设 PYTHONIOENCODING=utf-8，
否则 Windows 默认 GBK 会乱码）:
    {"pages": 12, "page_texts": ["第1页文本", "第2页文本", ...]}

错误信息一律输出到 stderr，退出码非 0。
"""

import json
import sys


def fail(msg: str, code: int = 1) -> None:
    print(f"pdf_extract error: {msg}", file=sys.stderr)
    sys.exit(code)


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: pdf_extract.py <file.pdf>", 2)
    path = sys.argv[1]

    try:
        # 本机 pymupdf >= 1.28 后旧名 `import fitz` 已弃用，统一用新名
        import pymupdf
    except ImportError:
        fail(
            "pymupdf not installed. install with: "
            "`pip install pymupdf` (Debian/Ubuntu PEP 668: use venv / pipx "
            "/ `pip install --break-system-packages pymupdf`)"
        )

    try:
        doc = pymupdf.open(path)
    except Exception as e:  # noqa: BLE001 — 子进程边界处统一转 stderr
        fail(f"cannot open {path!r}: {e}")

    if not doc.is_pdf:
        fail(f"{path!r} is not a valid PDF")

    page_texts = []
    for page in doc:
        try:
            page_texts.append(page.get_text())
        except Exception as e:  # noqa: BLE001
            fail(f"page extract failed: {e}")
    doc.close()

    json.dump(
        {"pages": len(page_texts), "page_texts": page_texts},
        sys.stdout,
        ensure_ascii=False,
    )


if __name__ == "__main__":
    main()
