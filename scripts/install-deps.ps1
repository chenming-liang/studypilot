# StudyPilot 依赖安装脚本（Windows）
# 安装 PDF 提取所需的 Python 3 + PyMuPDF。
# 用法：右键「使用 PowerShell 运行」，或在 PowerShell 中执行本脚本。
# 首次运行若提示执行策略限制：Set-ExecutionPolicy -Scope Process Bypass

$ErrorActionPreference = "Stop"
Write-Host "== StudyPilot 依赖安装 (Windows) =="

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
