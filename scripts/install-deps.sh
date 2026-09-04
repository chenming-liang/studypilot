#!/usr/bin/env bash
# StudyPilot 依赖安装脚本（Linux / macOS）
# 自动检测系统包管理器，安装 PDF 提取与剪贴板所需的运行时依赖。
# 用法：bash scripts/install-deps.sh

set -euo pipefail

echo "== 检测平台 =="
OS="$(uname -s)"
case "$OS" in
  Darwin) PLATFORM="macos" ;;
  Linux)  PLATFORM="linux" ;;
  *)      echo "不支持的平台: $OS"; exit 1 ;;
esac
echo "平台: $PLATFORM"

install_pip_pymupdf() {
  # 优先用系统包；PEP 668 (Debian 12+/Ubuntu 23.04+) 需 --break-system-packages
  if python3 -c "import fitz" 2>/dev/null; then
    echo "PyMuPDF 已安装"
  elif pip3 install pymupdf 2>/dev/null; then
    echo "PyMuPDF 已通过 pip 安装"
  else
    echo "尝试使用系统环境安装 PyMuPDF（PEP 668）..."
    pip3 install --break-system-packages pymupdf
  fi
}

case "$PLATFORM" in
  macos)
    # Python 3 + pbcopy（系统自带，无需装剪贴板工具）
    if ! command -v python3 >/dev/null; then
      echo "未检测到 python3，尝试 brew 安装..."
      if command -v brew >/dev/null; then
        brew install python@3
      else
        echo "请先安装 Homebrew（https://brew.sh），或手动安装 Python 3。"
        exit 1
      fi
    fi
    install_pip_pymupdf
    ;;

  linux)
    # 剪贴板：X11 用 xclip，Wayland 用 wl-clipboard；优先已有桌面环境对应者
    if ! command -v python3 >/dev/null; then
      echo "未检测到 python3，尝试安装..."
      if command -v apt-get >/dev/null; then
        sudo apt-get update && sudo apt-get install -y python3
      elif command -v dnf >/dev/null; then
        sudo dnf install -y python3
      elif command -v pacman >/dev/null; then
        sudo pacman -S --noconfirm python
      else
        echo "无法自动安装 Python 3，请手动安装后重试。"
        exit 1
      fi
    fi

    # PyMuPDF：优先系统包，避免跨 Python 版本编译问题
    if python3 -c "import fitz" 2>/dev/null; then
      echo "PyMuPDF 已安装"
    elif command -v apt-get >/dev/null; then
      sudo apt-get install -y python3-pymupdf || install_pip_pymupdf
    elif command -v dnf >/dev/null; then
      sudo dnf install -y python3-pymupdf || install_pip_pymupdf
    elif command -v pacman >/dev/null; then
      sudo pacman -S --noconfirm python-pymupdf || install_pip_pymupdf
    else
      install_pip_pymupdf
    fi

    # 剪贴板工具（拖选复制用；缺失不影响核心功能）
    if command -v xclip >/dev/null || command -v wl-copy >/dev/null; then
      echo "剪贴板工具已就绪"
    elif command -v apt-get >/dev/null; then
      sudo apt-get install -y xclip wl-clipboard
    elif command -v dnf >/dev/null; then
      sudo dnf install -y xclip wl-clipboard
    elif command -v pacman >/dev/null; then
      sudo pacman -S --noconfirm xclip wl-clipboard
    fi
    ;;
esac

echo
echo "== 完成 =="
python3 -c "import fitz; print('PyMuPDF OK')"
