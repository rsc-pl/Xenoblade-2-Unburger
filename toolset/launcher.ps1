$ErrorActionPreference = 'Stop'
$project = $PSScriptRoot
$manifest = Join-Path $project 'Cargo.toml'
$target = Join-Path $project 'target'
$binary = Join-Path $target 'release\xenoblade-bdat-studio.exe'
$stamp = Join-Path $target '.bdat-studio-sources.sha256'

if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
    [Console]::Error.WriteLine('ERROR: Cargo manifest not found.')
    exit 1
}
if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine('ERROR: Cargo not found in PATH.')
    exit 127
}

# Include both relative filenames and file contents; archive timestamps are not trusted.
$paths = @()
foreach ($folder in @('src', 'assets')) {
    $dir = Join-Path $project $folder
    if (Test-Path -LiteralPath $dir -PathType Container) {
        $paths += @(Get-ChildItem -LiteralPath $dir -File -Recurse | ForEach-Object { $_.FullName })
    }
}
foreach ($filename in @('Cargo.toml', 'build.rs')) {
    $path = Join-Path $project $filename
    if (Test-Path -LiteralPath $path -PathType Leaf) { $paths += $path }
}
$lines = foreach ($path in ($paths | Sort-Object)) {
    $relative = $path.Substring($project.Length).TrimStart([char[]]@('\', '/')).Replace('\', '/')
    $digest = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    "$relative=$digest"
}
$bytes = [Text.Encoding]::UTF8.GetBytes(($lines -join "`n"))
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $current = [BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
} finally { $sha.Dispose() }
$previous = ''
if (Test-Path -LiteralPath $stamp -PathType Leaf) {
    $previous = (Get-Content -LiteralPath $stamp -Raw).Trim()
}
New-Item -ItemType Directory -Force -Path $target | Out-Null

if ((Test-Path -LiteralPath $binary -PathType Leaf) -and $current -ne $previous) {
    Write-Host 'Source changes detected; rebuilding application package.'
    & cargo.exe clean --release --package xenoblade-bdat-studio --manifest-path $manifest --target-dir $target
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
& cargo.exe build --release --manifest-path $manifest --target-dir $target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    [Console]::Error.WriteLine('ERROR: Build finished but expected release executable is missing.')
    exit 1
}
[System.IO.File]::WriteAllText($stamp, "$current`n", [Text.Encoding]::UTF8)
& $binary @args
exit $LASTEXITCODE
