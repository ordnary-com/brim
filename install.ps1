# Installs brim on Windows from the GitHub releases.
#
#   irm https://brimlang.com/install.ps1 | iex
#
# $env:BRIM_VERSION picks a version (default: the latest release) and
# $env:BRIM_INSTALL the directory (default: %LOCALAPPDATA%\Programs\brim).
# $env:BRIM_NO_ICONS = "1" skips the file icons in File Explorer (`brim icons`).
$ErrorActionPreference = "Stop"
$repo = "ordnary-com/brim"

$version = $env:BRIM_VERSION
if (-not $version) {
    $releases = Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=1"
    $version = $releases[0].tag_name
}
$version = $version.TrimStart("v")

$base = "https://github.com/$repo/releases/download/v$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"
    # The build with native compilation (`brim build`) when the release has
    # one, else the interpreter and tools.
    $platform = "windows-x86_64-native"
    if (-not (Select-String -Path "$tmp\SHA256SUMS" -Pattern "brim-$version-$platform.zip" -SimpleMatch -Quiet)) {
        $platform = "windows-x86_64"
    }
    $archive = "brim-$version-$platform.zip"
    Write-Host "downloading brim $version ($platform)"
    Invoke-WebRequest "$base/$archive" -OutFile "$tmp\$archive"
    $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match " $([regex]::Escape($archive))$" }
    if (-not $line) { throw "$archive is not listed in SHA256SUMS" }
    $expected = ($line -split " ")[0]
    $actual = (Get-FileHash "$tmp\$archive" -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "the checksum of $archive does not match" }

    Expand-Archive "$tmp\$archive" -DestinationPath $tmp
    $dir = $env:BRIM_INSTALL
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA "Programs\brim" }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item "$tmp\brim-$version-$platform\brim.exe" "$dir\brim.exe" -Force

    $path = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($path -split ";") -notcontains $dir) {
        [Environment]::SetEnvironmentVariable("Path", "$dir;$path", "User")
        Write-Host "added $dir to your PATH; open a new terminal to use it"
    }
    Write-Host "installed $(& "$dir\brim.exe" --version) in $dir"
    if ($env:BRIM_NO_ICONS -ne "1" -and -not $env:CI) {
        & "$dir\brim.exe" icons
    }
    if ($platform -like "*-native" -and -not (Get-Command clang -ErrorAction SilentlyContinue)) {
        Write-Host "brim build needs clang and the Visual Studio C++ build tools to link; brim run works without them"
    }
} finally {
    Remove-Item -Recurse -Force $tmp
}
