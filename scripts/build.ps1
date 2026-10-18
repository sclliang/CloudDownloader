$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$running = Get-Process clouddownloader -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and $_.Path.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)
}
if ($running) { throw '请先从托盘菜单退出正在运行的 CloudDownloader，再执行打包。' }
Push-Location $projectRoot
try {
    if (-not (Test-Path 'node_modules')) {
        npm.cmd ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci 失败' }
    }
    cargo fmt --manifest-path src-tauri/Cargo.toml --check
    if ($LASTEXITCODE -ne 0) { throw 'Rust 格式检查失败' }
    cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Rust 测试失败' }
    node node_modules/@tauri-apps/cli/tauri.js build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw 'Tauri 构建失败' }
    & (Join-Path $PSScriptRoot 'package.ps1') -IncludeInstaller
} finally {
    Pop-Location
}
