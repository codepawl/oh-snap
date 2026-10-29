# Installs the latest oh-snap release for the current user. No admin rights needed.
#   irm https://raw.githubusercontent.com/codepawl/oh-snap/main/install.ps1 | iex
# Running it again updates oh-snap in place. After that oh-snap updates itself.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The releases/latest/download links need no GitHub API call, so they have no hourly limit.
$latest = 'https://github.com/codepawl/oh-snap/releases/latest/download'
$download = Join-Path ([IO.Path]::GetTempPath()) 'oh-snap-setup.exe'
Write-Host 'Downloading the latest oh-snap...'
Invoke-WebRequest "$latest/oh-snap.exe" -OutFile $download -UseBasicParsing

$checksums = [Text.Encoding]::UTF8.GetString((Invoke-WebRequest "$latest/SHA256SUMS.txt" -UseBasicParsing).Content)
$expected = ($checksums -split "`n" | Where-Object { $_ -match 'oh-snap\.exe' }) -replace '\s.*$', ''
$actual = (Get-FileHash $download -Algorithm SHA256).Hash
if (-not $expected -or $expected -ne $actual) {
    Remove-Item $download
    throw "Checksum mismatch: expected $expected, got $actual"
}

# `install` stops a running oh-snap, copies the program to %LOCALAPPDATA%\Programs\oh-snap
# and starts it. oh-snap is a GUI-subsystem program, so PowerShell must be told to wait.
$run = Start-Process $download -ArgumentList 'install' -Wait -PassThru -NoNewWindow
Remove-Item $download
if ($run.ExitCode -ne 0) {
    throw "oh-snap install failed with exit code $($run.ExitCode)"
}
Write-Host 'oh-snap is installed. Press PrtSc or Win+Shift+S to snip. It updates itself from now on.'
