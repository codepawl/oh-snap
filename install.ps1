# Installs the latest oh-snap release for the current user. No admin rights needed.
#   irm https://raw.githubusercontent.com/codepawl/oh-snap/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$release = Invoke-RestMethod 'https://api.github.com/repos/codepawl/oh-snap/releases/latest'
$asset = $release.assets | Where-Object { $_.name -eq 'oh-snap.exe' }
if (-not $asset) {
    throw "Release $($release.tag_name) has no oh-snap.exe"
}

$download = Join-Path ([IO.Path]::GetTempPath()) "oh-snap-$($release.tag_name).exe"
Write-Host "Downloading oh-snap $($release.tag_name)..."
Invoke-WebRequest $asset.browser_download_url -OutFile $download -UseBasicParsing

$checksums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' }
if ($checksums) {
    $expected = ((Invoke-RestMethod $checksums.browser_download_url) -split "`n" |
        Where-Object { $_ -match 'oh-snap\.exe' }) -replace '\s.*$', ''
    $actual = (Get-FileHash $download -Algorithm SHA256).Hash
    if ($expected -and $expected -ne $actual) {
        Remove-Item $download
        throw "Checksum mismatch: expected $expected, got $actual"
    }
}

# `install` copies the program to %LOCALAPPDATA%\Programs\oh-snap and starts it.
# oh-snap is a GUI-subsystem program, so PowerShell must be told to wait for it.
$run = Start-Process $download -ArgumentList 'install' -Wait -PassThru -NoNewWindow
Remove-Item $download
if ($run.ExitCode -ne 0) {
    throw "oh-snap install failed with exit code $($run.ExitCode)"
}
Write-Host 'oh-snap is installed. Press PrtSc or Win+Shift+S to snip.'
