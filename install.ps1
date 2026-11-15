<#
.SYNOPSIS
    poly installer for Windows — downloads the correct prebuilt binaries.

.DESCRIPTION
    Installs poly. Re-run any time to UPDATE to the latest release (it overwrites in place).

        irm https://raw.githubusercontent.com/Goldziher/polylint/main/install.ps1 | iex

    Pin a version or change the install dir with environment variables before running:
        $env:POLY_VERSION = "v0.1.5"
        $env:POLY_INSTALL_DIR = "C:\tools\poly"
        $env:POLY_NO_MODIFY_PATH = "1"
#>

$ErrorActionPreference = "Stop"

$Repo = "Goldziher/polylint"
$Binaries = @("poly.exe")

$Version = if ($env:POLY_VERSION) { $env:POLY_VERSION } else { "latest" }
$InstallDir = if ($env:POLY_INSTALL_DIR) { $env:POLY_INSTALL_DIR } else { "$env:LOCALAPPDATA\poly\bin" }

function Info($msg) { Write-Host "✓ $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "⚠ $msg" -ForegroundColor Yellow }
function Die($msg) { Write-Host "✗ $msg" -ForegroundColor Red; exit 1 }

# Install $Source at $Destination without ever leaving $Destination absent or
# half-written: copy into a staging file beside it (same volume), then replace the
# destination in one filesystem operation. poly's git hooks are installed per
# repository but resolve `poly` from PATH globally and fail closed, so an upgrade
# that deletes or truncates the destination blocks every commit on the machine for
# as long as it runs.
function Install-Binary($Source, $Destination) {
    $staged = "$Destination.tmp.$PID"
    try {
        Copy-Item -LiteralPath $Source -Destination $staged -Force
        if (Test-Path -LiteralPath $Destination) {
            try {
                # ReplaceFile: the destination is swapped for the staged copy in
                # one step (no delete-then-move window).
                [System.IO.File]::Replace($staged, $Destination, $null)
            } catch {
                # Windows refuses to overwrite a binary that is currently running,
                # but it does allow renaming one out of the way. Park the old copy
                # aside, move the new one in, and delete the old one afterwards
                # (it stays until the running process exits).
                $parked = "$Destination.old-$([System.Guid]::NewGuid().ToString('N'))"
                Move-Item -LiteralPath $Destination -Destination $parked -Force
                Move-Item -LiteralPath $staged -Destination $Destination -Force
                Remove-Item -LiteralPath $parked -Force -ErrorAction SilentlyContinue
            }
        } else {
            Move-Item -LiteralPath $staged -Destination $Destination -Force
        }
    } finally {
        if (Test-Path -LiteralPath $staged) {
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
        }
    }
}


# Install `polylint.exe` beside `poly.exe` as an alias for the same tool, not a
# second binary: the package is published as `polylint` on PyPI and
# `@polylint/polylint` on npm (the unscoped `poly` name is taken on both), so
# someone who installed it under that name will reasonably type `polylint`.
#
# A hard link is preferred over a symlink because creating a symlink on Windows
# needs either Developer Mode or an elevated shell, and over a `.cmd` shim
# because a batch wrapper intercepts Ctrl-C with "Terminate batch job (Y/N)?"
# and mangles argument quoting. A hard link costs nothing and behaves exactly
# like the executable. Where the volume refuses one (FAT32, some network
# shares), fall back to a copy — the installer rewrites the alias on every run,
# so the copy cannot outlive the upgrade that replaced poly.exe.
function Install-Alias($Target, $AliasPath) {
    if (-not (Test-Path -LiteralPath $Target)) { Warn "Cannot create the polylint alias: $Target is missing"; return }
    # poly.exe is already replaced by now, so a locked alias (a running polylint.exe)
    # must not fail the install: warn and keep the previous alias.
    try {
        Remove-Item -LiteralPath $AliasPath -Force -ErrorAction Stop
    } catch {
        if (Test-Path -LiteralPath $AliasPath) {
            Warn "Could not replace the polylint alias at ${AliasPath} (in use?); close it and re-run to refresh it: $_"
            return
        }
    }
    try {
        New-Item -ItemType HardLink -Path $AliasPath -Target $Target -ErrorAction Stop | Out-Null
    } catch {
        try {
            Copy-Item -LiteralPath $Target -Destination $AliasPath -Force -ErrorAction Stop
            Warn "hard links are unavailable here, so polylint.exe was installed as a copy of poly.exe"
        } catch {
            Warn "Failed to install the polylint alias at ${AliasPath}: $_"
        }
    }
}

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { Die "No prebuilt binary for Windows arm64; build from source: https://github.com/$Repo#installation" }
    default { Die "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
$target = "$arch-pc-windows-msvc"
$ext = "zip"
Info "Platform: $target"

if ($Version -eq "latest") {
    Info "Resolving latest release..."
    $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" `
        -MaximumRedirection 0 -ErrorAction SilentlyContinue
    $loc = $resp.Headers.Location
    if ($loc -and $loc -match "/tag/(?<tag>[^/]+)$") {
        $Version = $Matches.tag
    } else {
        $api = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $api.tag_name
    }
    if (-not $Version) { Die "Could not resolve the latest release" }
}

# X.Y.Z[-pre.release]; \A and \z so a trailing newline cannot slip through.
$bare = if ($Version.StartsWith("v")) { $Version.Substring(1) } else { $Version }
if ($bare -cnotmatch '\A[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?\z') {
    $shown = $Version -replace '[^0-9A-Za-z.+-]', '?'
    Die "invalid version: $shown"
}

$tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
$ver = $tag.TrimStart("v")
$asset = "poly-$ver-$target.$ext"
$base = "https://github.com/$Repo/releases/download/$tag"

$existing = Join-Path $InstallDir "poly.exe"
if (Test-Path $existing) {
    # `poly --version` prints `poly 0.35.0 (release build v0.35.0, release)`.
    $prev = ((& $existing --version 2>$null) -split '\s+')[1]
    if ($prev -notmatch '^\d+\.\d+\.\d+([.+-][0-9A-Za-z.+-]+)?$') { $prev = $null }
    if ($prev -eq $ver) { Info "poly $ver already installed — re-installing." }
    elseif ($prev) { Info "Updating poly $prev -> $ver" }
} else {
    Info "Installing poly $ver"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("poly-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $archive = Join-Path $tmp $asset
    Info "Downloading $base/$asset"
    try {
        Invoke-WebRequest -Uri "$base/$asset" -OutFile $archive
    } catch {
        Die "Failed to download $base/${asset}: check the version and your network (does release $tag ship $target?)"
    }

    $sumsPath = Join-Path $tmp "sha256sums.txt"
    try {
        Invoke-WebRequest -Uri "$base/sha256sums.txt" -OutFile $sumsPath
    } catch {
        Die "Could not download $base/sha256sums.txt - refusing to install unverified binaries"
    }
    $expected = $null
    foreach ($line in Get-Content $sumsPath) {
        $parts = $line -split '\s+'
        if ($parts.Count -ge 2) {
            $name = $parts[-1] -replace '^\*', '' -replace '^\./', ''
            if ($name -eq $asset) { $expected = $parts[0].ToLower() }
        }
    }
    if (-not $expected) { Die "No checksum entry for $asset — refusing to install unverified binaries" }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLower()
    if ($actual -ne $expected) { Die "Checksum mismatch for $asset (expected $expected, got $actual)" }
    Info "Checksum verified"

    Expand-Archive -Path $archive -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    # [System.IO.File] resolves relative paths against the process working
    # directory, not PowerShell's location, so pin the install dir to a full path.
    $InstallDir = (Resolve-Path -LiteralPath $InstallDir).Path
    foreach ($binary in $Binaries) {
        $src = Join-Path $tmp $binary
        if (-not (Test-Path $src)) { Die "Expected binary $binary missing from $asset" }
        Install-Binary $src (Join-Path $InstallDir $binary)
    }
    Install-Alias (Join-Path $InstallDir "poly.exe") (Join-Path $InstallDir "polylint.exe")
    Info "Installed poly (and the polylint alias) -> $InstallDir"
    Info "Engines download as signed components on first use; run 'poly components sync' in a repository to fetch them ahead (offline or CI)."
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';') -notcontains $InstallDir) {
    if ($env:POLY_NO_MODIFY_PATH) {
        Warn "$InstallDir is not on your PATH. Add it via System > Environment Variables."
    } else {
        $newPath = if ($userPath) { "$($userPath.TrimEnd(';'));$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $env:Path = "$env:Path;$InstallDir"
        Info "Added $InstallDir to your user PATH (restart your terminal to pick it up)."
    }
}

Write-Host "poly $ver is ready. Run 'poly --help' to get started (polylint is an alias for poly)." -ForegroundColor Green
