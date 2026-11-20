# Install the oxoffice command line on Windows.
#
#   powershell -ExecutionPolicy Bypass -c "irm https://github.com/oxoffice-dev/oxoffice/releases/latest/download/install.ps1 | iex"
#
# Environment:
#   OXOFFICE_VERSION          version to install, such as 0.1.0 (default: the latest release)
#   OXOFFICE_HOME             install root (default: %USERPROFILE%\.oxoffice), the binary goes to bin\
#   OXOFFICE_NO_PATH_UPDATE   set to any value to leave the user PATH alone
#
# The archive is checked against the release's SHA256SUMS before anything is
# installed. When the GitHub CLI is available and signed in, the archive's
# build provenance is verified too (gh attestation verify).

$ErrorActionPreference = 'Stop'
$Repo = 'oxoffice-dev/oxoffice'
$HomeDir = if ($env:OXOFFICE_HOME) { $env:OXOFFICE_HOME } else { Join-Path $env:USERPROFILE '.oxoffice' }
$BinDir = Join-Path $HomeDir 'bin'

function Say($Message) { Write-Host "oxoffice: $Message" }
function Fail($Message) { throw "oxoffice: error: $Message" }

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($Arch -ne 'X64') { Fail "unsupported architecture $Arch, only x86_64 Windows builds exist" }
$Target = 'x86_64-pc-windows-msvc'

$Version = $env:OXOFFICE_VERSION
if (-not $Version) {
    $Response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 5 -UseBasicParsing
    $Final = @($Response.BaseResponse.ResponseUri, $Response.BaseResponse.RequestMessage.RequestUri | Where-Object { $_ })[0].ToString()
    if ($Final -notmatch '/tag/v(.+)$') { Fail 'could not find the latest release' }
    $Version = $Matches[1]
}
$Version = $Version.TrimStart('v')

$Archive = "oxoffice-$Version-$Target.zip"
$Base = "https://github.com/$Repo/releases/download/v$Version"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("oxoffice-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null

try {
    Say "downloading oxoffice $Version for $Target"
    Invoke-WebRequest -Uri "$Base/$Archive" -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing
    Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile (Join-Path $Tmp 'SHA256SUMS') -UseBasicParsing

    $Line = Get-Content (Join-Path $Tmp 'SHA256SUMS') | Where-Object { ($_ -split '\s+')[1].TrimStart('*') -eq $Archive }
    if (-not $Line) { Fail "$Archive is not listed in SHA256SUMS" }
    $Expected = ($Line -split '\s+')[0].ToLower()
    $Actual = (Get-FileHash -Algorithm SHA256 (Join-Path $Tmp $Archive)).Hash.ToLower()
    if ($Actual -ne $Expected) { Fail "checksum mismatch for $Archive, nothing installed" }
    Say 'checksum verified'

    if (Get-Command gh -ErrorAction SilentlyContinue) {
        gh auth status *> $null
        if ($LASTEXITCODE -eq 0) {
            gh attestation verify (Join-Path $Tmp $Archive) --repo $Repo *> $null
            if ($LASTEXITCODE -ne 0) { Fail "build provenance of $Archive could not be verified, nothing installed" }
            Say 'build provenance verified'
        }
    }

    Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    Copy-Item (Join-Path $Tmp "oxoffice-$Version-$Target\oxoffice.exe") (Join-Path $BinDir 'oxoffice.exe') -Force
    Say "installed $(& (Join-Path $BinDir 'oxoffice.exe') --version) in $BinDir"
}
finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($UserPath -split ';') -notcontains $BinDir) {
    if ($env:OXOFFICE_NO_PATH_UPDATE) {
        Say "add $BinDir to your PATH"
    }
    else {
        [Environment]::SetEnvironmentVariable('Path', "$BinDir;$UserPath", 'User')
        Say "added $BinDir to your user PATH, open a new terminal to use oxoffice"
    }
}
