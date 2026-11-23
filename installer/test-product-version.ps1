# Check Windows Installer upgrade ordering across stable Orvia releases.
$ErrorActionPreference = 'Stop'
$previous = [version]'0.3.1'
foreach ($case in @(
    @{ Series = '0.4'; Version = '0.1.0' },
    @{ Series = '0.5'; Version = '0.1.1' },
    @{ Series = '0.6'; Version = '0.1.2' },
    @{ Series = '0.7'; Version = '0.1.3' },
    @{ Series = '0.8'; Version = '0.1.4' },
    @{ Series = '0.9'; Version = '0.1.5' },
    @{ Series = '0.10'; Version = '0.1.6' }
)) {
    $current = [version](& "$PSScriptRoot/product-version.ps1" @case)
    if ($current -le $previous) { throw "Upgrade ordering failed for $($case.Version)" }
    $previous = $current
}
foreach ($invalid in '0.1', '0.1.0-beta.1') {
    try { & "$PSScriptRoot/product-version.ps1" -Series '0.4' -Version $invalid }
    catch { continue }
    throw "Unexpectedly accepted '$invalid'"
}
