# Installs brim on Windows from the GitHub releases.
#
#   irm https://raw.githubusercontent.com/ordnary-com/brim/main/install.ps1 | iex
#
# $env:BRIM_VERSION picks a version (default: the latest release) and
# $env:BRIM_INSTALL the directory (default: %LOCALAPPDATA%\Programs\brim).
$ErrorActionPreference = "Stop"
$repo = "ordnary-com/brim"

$version = $env:BRIM_VERSION
if (-not $version) {
    $releases = Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=1"
    $version = $releases[0].tag_name
}
$version = $version.TrimStart("v")

$archive = "brim-$version-windows-x86_64.zip"
$base = "https://github.com/$repo/releases/download/v$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Write-Host "downloading brim $version (windows-x86_64)"
    Invoke-WebRequest "$base/$archive" -OutFile "$tmp\$archive"
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"
    $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match " $([regex]::Escape($archive))$" }
    if (-not $line) { throw "$archive is not listed in SHA256SUMS" }
    $expected = ($line -split " ")[0]
    $actual = (Get-FileHash "$tmp\$archive" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "the checksum of $archive does not match" }

    Expand-Archive "$tmp\$archive" -DestinationPath $tmp
    $dir = $env:BRIM_INSTALL
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA "Programs\brim" }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item "$tmp\brim-$version-windows-x86_64\brim.exe" "$dir\brim.exe" -Force

    $path = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($path -split ";") -notcontains $dir) {
        [Environment]::SetEnvironmentVariable("Path", "$dir;$path", "User")
        Write-Host "added $dir to your PATH; open a new terminal to use it"
    }
    Write-Host "installed $(& "$dir\brim.exe" --version) in $dir"
} finally {
    Remove-Item -Recurse -Force $tmp
}
