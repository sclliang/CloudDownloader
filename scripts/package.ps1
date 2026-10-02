$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$configuration = Get-Content -LiteralPath (Join-Path $projectRoot 'src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$version = $configuration.version
$releaseRoot = Join-Path $projectRoot 'release'
$portableRoot = [IO.Path]::GetFullPath((Join-Path $releaseRoot "CloudDownloader-$version-win-x64"))
if (-not $portableRoot.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw '发布目录必须位于项目内'
}
$buildRoot = Join-Path $projectRoot 'src-tauri\target\release'
$executable = Join-Path $buildRoot 'clouddownloader.exe'
$metadata = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
if ($metadata.LegalCopyright -ne 'Copyright (c) 2026 sixunited. Author: Sclliang') {
    throw '主程序版权元数据检查失败'
}
New-Item -ItemType Directory -Path $portableRoot -Force | Out-Null
Copy-Item -LiteralPath $executable -Destination (Join-Path $portableRoot 'CloudDownloader.exe')
Copy-Item -LiteralPath (Join-Path $buildRoot 'aria2c.exe') -Destination $portableRoot
Copy-Item -LiteralPath (Join-Path $projectRoot 'third-party\notices') -Destination $portableRoot -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination $portableRoot
if (Test-Path -LiteralPath (Join-Path $projectRoot 'VERIFICATION.md')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot 'VERIFICATION.md') -Destination $portableRoot
}
if (Test-Path -LiteralPath (Join-Path $projectRoot 'docs')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs') -Destination $portableRoot -Recurse -Force
}
$installerPath = Join-Path $buildRoot "bundle\nsis\CloudDownloader_${version}_x64-setup.exe"
if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) { throw '找不到当前版本安装包' }
$installer = Get-Item -LiteralPath $installerPath
Copy-Item -LiteralPath $installer.FullName -Destination (Join-Path $releaseRoot "CloudDownloader_${version}_x64-setup.exe")
$zipPath = Join-Path $releaseRoot "CloudDownloader-$version-win-x64.zip"
Compress-Archive -Path (Join-Path $portableRoot '*') -DestinationPath $zipPath -Force
Get-ChildItem -LiteralPath $releaseRoot -File | Select-Object Name, Length
