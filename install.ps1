# Install AImeter and wire it into your Claude Code statusline, on Windows.
#
# Nothing here overwrites anything. An existing statusLine setting is left exactly
# as it is, settings.json is copied before it is touched, and running this twice
# does nothing the second time.
#
#   irm https://raw.githubusercontent.com/MarioPayan/AImeter/main/install.ps1 | iex
#
# Piping into iex cannot pass arguments. To install the binary without wiring
# anything, save the file and run:  .\install.ps1 -NoWire
#
#   $env:AIMETER_DEST   install the binary elsewhere (default ~\.local\bin)

[CmdletBinding()]
param([switch]$NoWire)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest spends more time drawing its progress bar than downloading.
$ProgressPreference = 'SilentlyContinue'

$Repo = 'MarioPayan/AImeter'
$Home_ = $env:USERPROFILE
$Dest = if ($env:AIMETER_DEST) { $env:AIMETER_DEST } else { Join-Path $Home_ '.local\bin' }
$ClaudeDir = Join-Path $Home_ '.claude'
$Settings = Join-Path $ClaudeDir 'settings.json'

# ── the binary ───────────────────────────────────────────────────────────────
# One target for now. Windows on ARM runs x64 binaries under emulation, so this
# is the right file there too until that stops being true.
$target = 'x86_64-pc-windows-msvc'
$base = "https://github.com/$Repo/releases/latest/download/aimeter-$target.tar.gz"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("aimeter-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp > $null
try {
    Write-Host "aimeter: fetching $target"
    $archive = Join-Path $tmp 'aimeter.tar.gz'
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $base -OutFile $archive
        Invoke-WebRequest -UseBasicParsing -Uri "$base.sha256" -OutFile "$archive.sha256"
    } catch {
        Write-Error ("aimeter: download failed. Build it instead:`n" +
                     "  cargo install --git https://github.com/$Repo")
    }

    # Verify against the published checksum before unpacking anything. A mismatch
    # is a hard stop — a truncated download and a tampered one look the same from
    # here. Get-FileHash is built in, so unlike the POSIX installer there is no
    # machine that has to be let through without a check.
    $expected = ((Get-Content "$archive.sha256" -Raw) -split '\s+')[0]
    $actual = (Get-FileHash $archive -Algorithm SHA256).Hash
    if ($actual -ne $expected) {
        Write-Error ("aimeter: checksum mismatch — the download is corrupt or not what was published.`n" +
                     "         expected $expected`n         got      $actual")
    }

    # tar has shipped with Windows since 2018, which is why the release is a
    # tarball on every platform instead of a zip on one of them.
    tar -xzf $archive -C $tmp
    if ($LASTEXITCODE -ne 0) { Write-Error "aimeter: could not unpack the archive" }

    New-Item -ItemType Directory -Force -Path $Dest > $null
    $exe = Join-Path $Dest 'aimeter.exe'
    Move-Item -Force (Join-Path $tmp 'aimeter.exe') $exe
    Write-Host "aimeter: installed $(& $exe --version) to $exe"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# The statusline runs through cmd.exe, where a quoted path is how a directory with
# a space in it survives. No wrapper script: pointing straight at the binary is one
# less file, and Claude Code pipes the payload to it either way.
$command = '"{0}" line' -f $exe

if ($NoWire) {
    Write-Host ""
    Write-Host "Add this to $Settings yourself:"
    Write-Host ""
    Write-Host "  `"statusLine`": { `"type`": `"command`", `"command`": $(ConvertTo-Json $command) }"
    Write-Host ""
    exit 0
}

# ── settings.json ────────────────────────────────────────────────────────────
# Only ever adds statusLine when there is none. Someone who already has one has
# made a choice, and an installer that overrules it is a bug.
New-Item -ItemType Directory -Force -Path $ClaudeDir > $null
$data = [pscustomobject]@{}
if (Test-Path $Settings) {
    try {
        $data = (Get-Content $Settings -Raw) | ConvertFrom-Json
    } catch {
        Write-Host "aimeter: settings.json is not readable JSON — add statusLine yourself:"
        Write-Host "  `"statusLine`": { `"type`": `"command`", `"command`": $(ConvertTo-Json $command) }"
        exit 0
    }
    if ($data.PSObject.Properties.Name -contains 'statusLine') {
        Write-Host "aimeter: settings.json already sets statusLine — left alone"
        exit 0
    }
    Copy-Item $Settings "$Settings.before-aimeter" -Force
}

$statusLine = [pscustomobject]@{ type = 'command'; command = $command; refreshInterval = 10 }
$data | Add-Member -NotePropertyName statusLine -NotePropertyValue $statusLine -Force
# -Depth, because PowerShell's default of 2 silently flattens anything nested
# deeper than that into a string — in someone else's settings file.
$json = $data | ConvertTo-Json -Depth 100
$tmpSettings = "$Settings.tmp"
Set-Content -Path $tmpSettings -Value $json -Encoding UTF8
Move-Item -Force $tmpSettings $Settings
Write-Host "aimeter: set statusLine in $Settings"

if (($env:PATH -split ';') -notcontains $Dest) {
    Write-Host "aimeter: note — $Dest is not on your PATH (the statusline uses the full path, so it works anyway)"
}

Write-Host ""
Write-Host "The model-scoped limit comes from an endpoint Anthropic does not document, which"
Write-Host "means reading the OAuth token in ~\.claude\.credentials.json. It is never written,"
Write-Host "logged or stored, and AIMETER_NO_FETCH=1 turns it off — the rest still works."
Write-Host "https://github.com/$Repo/blob/main/docs/how-it-works.md"
Write-Host ""
Write-Host "aimeter: done. Open a new Claude Code session to see it."
