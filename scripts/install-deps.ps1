# StudyPilot 依赖安装脚本（Windows）
# 安装 PDF 提取所需的 Python 3 + PyMuPDF，并引导使用 MSVC Rust toolchain。
# 用法：右键「使用 PowerShell 运行」，或在 PowerShell 中执行本脚本。
# 首次运行若提示执行策略限制：Set-ExecutionPolicy -Scope Process Bypass

$ErrorActionPreference = "Stop"
# 统一控制台输出编码为 UTF-8，避免中文提示乱码（Windows PowerShell 默认本地代码页）
try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
} catch {
    Write-Host "无法设置控制台编码（忽略）"
}
Write-Host "== StudyPilot 依赖安装 (Windows) =="

# ── 0. Rust toolchain：推荐 MSVC（GNU toolchain 在含中文的路径下链接会失败）──
$hostTriple = rustup show active-toolchain 2>$null
if ($LASTEXITCODE -eq 0 -and $hostTriple -match "pc-windows-gnu") {
    Write-Host ""
    Write-Host "检测到 GNU toolchain（$hostTriple）。"
    Write-Host "GNU 工具链在含中文的路径下链接会失败，建议切换到 MSVC："
    Write-Host "  1. 安装 Visual Studio Build Tools（含 C++ 桌面开发）"
    Write-Host "     winget install Microsoft.VisualStudio.2022.BuildTools"
    Write-Host "  2. rustup toolchain install stable-x86_64-pc-windows-msvc"
    Write-Host "  3. rustup default stable-x86_64-pc-windows-msvc"
    Write-Host ""
}

# 检查 python / python3 / py
function Get-Python {
    foreach ($cmd in @("python", "python3", "py")) {
        try {
            & $cmd --version 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) { return $cmd }
        } catch {}
    }
    return $null
}

$py = Get-Python
if (-not $py) {
    Write-Host "未检测到 Python。正在尝试用 winget 安装..."
    $installed = $false
    try {
        winget install -e --id Python.Python.3.12 --silent
        $installed = $true
    } catch {
        Write-Host "winget 安装失败，请手动安装 Python 3：https://www.python.org/downloads/"
    }
    if (-not $installed) { exit 1 }
    $py = Get-Python
    if (-not $py) { exit 1 }
}

Write-Host "使用解释器: $py"

# 安装 PyMuPDF
try {
    & $py -m pip install pymupdf
    Write-Host ""
    Write-Host "== 完成 =="
    & $py -c "import fitz; print('PyMuPDF OK')"
} catch {
    Write-Host "pip 安装失败，请手动执行: $py -m pip install pymupdf"
    exit 1
}
