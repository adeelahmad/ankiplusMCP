# One-command AnkiPlus install for students on Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install.ps1 | iex
#
# Installs what is missing (uv, Node, Tesseract) with winget, registers AnkiPlus in your
# AI app, downloads the matching model once, and runs a health check. Windows is less tested
# than macOS and Linux.
$ErrorActionPreference = 'Stop'
$Host_ = if ($env:ANKIPLUS_HOST) { $env:ANKIPLUS_HOST } else { 'claude-desktop' }
$Lectures = if ($env:ANKIPLUS_LECTURES) { $env:ANKIPLUS_LECTURES } else { Join-Path $HOME 'Documents\Lectures' }
$Ref = if ($env:ANKIPLUS_REF) { $env:ANKIPLUS_REF } else { 'github:adeelahmad/ankiplusMCP#v0.4.0' }
function Have($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }
function Refresh { $env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User') }
$accept = @('--accept-source-agreements','--accept-package-agreements','-e','--silent')

Write-Host "`n==> 1/5 uv"
if (-not (Have uv)) { winget install --id astral-sh.uv @accept; Refresh }
Write-Host "`n==> 2/5 Node.js"
if (-not (Have npx)) { winget install --id OpenJS.NodeJS.LTS @accept; Refresh }
Write-Host "`n==> 3/5 Tesseract (OCR)"
if (-not (Have tesseract)) {
  winget install --id UB-Mannheim.TesseractOCR @accept
  $env:Path += ';C:\Program Files\Tesseract-OCR'
  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path','User') + ';C:\Program Files\Tesseract-OCR', 'User')
}
Write-Host "`n==> 4/5 Register AnkiPlus in $Host_ (lectures: $Lectures)"
New-Item -ItemType Directory -Force -Path $Lectures | Out-Null
npx -y $Ref init --host $Host_ --source-root $Lectures --yes
Write-Host "`n==> 5/5 Preview browser, matching model and health check"
npx -y $Ref setup --yes --model
npx -y $Ref doctor
Write-Host "`nDone. Restart $Host_. Put lecture PDFs and your AnKing .apkg export in $Lectures."
