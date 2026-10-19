param([switch]$SkipChecks)
$ErrorActionPreference = 'Continue' # Cargo writes progress to stderr; check its exit code below.
$root = Split-Path -Parent $PSScriptRoot
$rust = Join-Path $root 'rust'
Push-Location $rust
try {
    if (!$SkipChecks) {
        & cargo test --workspace
        if ($LASTEXITCODE -ne 0) { throw 'Workspace tests failed' }
        & cargo clippy --workspace --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
        & cargo fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
    }
    # A dedicated target directory keeps the portable static-CRT build separate.
    $oldFlags = $env:RUSTFLAGS
    try {
        $env:RUSTFLAGS = (($oldFlags + ' -C target-feature=+crt-static').Trim())
        & cargo build --locked --release --target x86_64-pc-windows-msvc --target-dir target\portable -p wh-desktop
        if ($LASTEXITCODE -ne 0) { throw 'Portable release build failed' }
    } finally { $env:RUSTFLAGS = $oldFlags }
    $out = Join-Path $root 'dist\WallHavenStudio-0.1.0-windows-x64'
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    $file = Join-Path $out 'WallHavenStudio.exe'
    $source = Join-Path $rust 'target\portable\x86_64-pc-windows-msvc\release\wh-desktop.exe'
    Copy-Item -LiteralPath $source -Destination $file -Force -ErrorAction Stop
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $file -ErrorAction Stop
    if ($hash.Hash -ne (Get-FileHash -Algorithm SHA256 -LiteralPath $source -ErrorAction Stop).Hash) {
        throw 'Packaged executable does not match the new build'
    }
    Write-Output "EXE=$file"
    Write-Output "BYTES=$((Get-Item -LiteralPath $file).Length)"
    Write-Output "SHA256=$($hash.Hash)"
    Write-Output 'Unsigned Windows x64 single-EXE build. Website login requires the system Microsoft Edge WebView2 Runtime.'
} finally { Pop-Location }
