$ErrorActionPreference = "Stop"

Write-Host "Building release binaries..."
cargo build --release

$distDir = Join-Path $PSScriptRoot "dist"
$stageDir = Join-Path $distDir "SimpleSerialMonitor"
$zipPath = Join-Path $distDir "SimpleSerialMonitor-portable-win64.zip"

if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null

Copy-Item (Join-Path $PSScriptRoot "target\release\ssm.exe") $stageDir
Copy-Item (Join-Path $PSScriptRoot "target\release\ssm-cli.exe") $stageDir
Copy-Item (Join-Path $PSScriptRoot "README.md") $stageDir
Copy-Item (Join-Path $PSScriptRoot "README.ja.md") $stageDir

Compress-Archive -Path $stageDir -DestinationPath $zipPath -CompressionLevel Optimal

$ssmSize = [math]::Round((Get-Item (Join-Path $stageDir "ssm.exe")).Length / 1KB, 1)
$cliSize = [math]::Round((Get-Item (Join-Path $stageDir "ssm-cli.exe")).Length / 1KB, 1)
$zipSize = [math]::Round((Get-Item $zipPath).Length / 1KB, 1)

Write-Host ""
Write-Host "=== Portable Package Ready ==="
Write-Host "  ssm.exe     : $ssmSize KB"
Write-Host "  ssm-cli.exe : $cliSize KB"
Write-Host "  ZIP archive : $zipPath ($zipSize KB)"
