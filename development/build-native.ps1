param([switch]$Candidate)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Manifest = Join-Path $Root 'rust/Cargo.toml'
$Output = Join-Path $Root $(if ($Candidate) { 'dist/candidate' } else { 'dist/native' })
Push-Location $Root
try {
    cargo fmt --manifest-path $Manifest --all
    if ($LASTEXITCODE -ne 0) { throw 'Formatting failed' }
    cargo fmt --manifest-path $Manifest --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
    cargo test --locked --manifest-path $Manifest --workspace
    if ($LASTEXITCODE -ne 0) { throw 'Native tests failed' }
    cargo build --release --locked --manifest-path $Manifest -p snapgent
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    New-Item -ItemType Directory -Force -Path $Output | Out-Null
    Copy-Item (Join-Path $Root 'rust/target/release/snapgent.exe') (Join-Path $Output 'Snapgent.exe') -Force
    Get-Item (Join-Path $Output 'Snapgent.exe') | Select-Object FullName, Length
    Get-FileHash (Join-Path $Output 'Snapgent.exe') -Algorithm SHA256
} finally {
    Pop-Location
}
