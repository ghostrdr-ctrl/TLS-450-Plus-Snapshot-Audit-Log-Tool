# Release build with nothing from this PC inside the exe.
#
# A normal build embeds the full source paths of every crate (for panic
# messages), and those paths contain the Windows user folder. They are
# rewritten here to neutral prefixes, the debug-file (PDB) reference is left
# out, and the finished exe is scanned for the user name and folder paths.
#
# Usage:  powershell -ExecutionPolicy Bypass -File build.ps1
# Output: dist\TLS-450-Plus-Snapshot-Audit-Log-Tool.exe

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$name = "TLS-450-Plus-Snapshot-Audit-Log-Tool"

# The version lives in Cargo.toml only; the window title and the exe's file
# properties read it. docs\CHANGELOG.md must have a section for it.
$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not (Select-String -Path docs\CHANGELOG.md -Pattern "^## $([regex]::Escape($version))$" -Quiet)) {
    throw "docs\CHANGELOG.md has no '## $version' section. Add one before building."
}

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE ".rustup" }

$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
$targetFwd = $target.Replace("\", "/")

# Most specific prefixes first. The target folder appears in code that build
# scripts generate (OUT_DIR), in either slash style.
$remaps = @(
    "--remap-path-prefix=$target=target",
    "--remap-path-prefix=$targetFwd=target",
    "--remap-path-prefix=$cargoHome=cargo",
    "--remap-path-prefix=$rustupHome=rustup",
    "--remap-path-prefix=$PSScriptRoot=app",
    "--remap-path-prefix=$env:USERPROFILE=home"
)
$env:RUSTFLAGS = ($remaps + @("-C", "link-arg=/DEBUG:NONE")) -join " "

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = Join-Path $target "release\$name.exe"

# Scan for anything from this PC: user name, user folder, project folder.
$bytes = [System.IO.File]::ReadAllBytes($exe)
$ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
$utf16 = [System.Text.Encoding]::Unicode.GetString($bytes)
$needles = @($env:USERNAME, $env:USERPROFILE, $PSScriptRoot, $target, $targetFwd, ".pdb") | Where-Object { $_ }
$found = @()
foreach ($n in $needles) {
    if ($ascii.IndexOf($n, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $utf16.IndexOf($n, [StringComparison]::OrdinalIgnoreCase) -ge 0) { $found += $n }
}
if ($found.Count -gt 0) { throw "The exe still contains: $($found -join ', ')" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item $exe dist\ -Force
$stamped = (Get-Item $exe).VersionInfo.ProductVersion
if ($stamped -ne $version) { throw "The exe's file version is '$stamped', expected '$version'" }
Write-Host "Clean build $version`: dist\$name.exe ($([math]::Round((Get-Item $exe).Length / 1MB, 1)) MB)"
