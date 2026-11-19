# Windows Installer compares three numeric fields. Each target release gets one
# series, with its stable build ordered after every snapshot in that series.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Series
)
$ErrorActionPreference = 'Stop'
if ($Series -notmatch '^(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})$' -or
    [int]$Matches[1] -gt 255 -or [int]$Matches[2] -gt 255) {
    throw "Invalid Windows Installer series '$Series'. Each field must be between 0 and 255."
}
if ($Version -cnotmatch '^\d+\.\d+\.\d+(?:-snapshot\.([1-9][0-9]*))?$') {
    throw "Expected a release or a numbered snapshot, got '$Version'."
}
$build = 65535
if ($Matches[1]) {
    $build = 0
    if (![int]::TryParse($Matches[1], [ref]$build) -or $build -ge 65535) {
        throw 'The snapshot number must be between 1 and 65534.'
    }
}
"$Series.$build"
