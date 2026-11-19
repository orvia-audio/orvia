# Cover Windows upgrade ordering across the old development MSIs, snapshots,
# and stable releases in successive installer series.
$ErrorActionPreference = 'Stop'
$previous = [version]'0.3.1'
foreach ($case in @(
    @{ Series = '0.4'; Version = '0.1.0-snapshot.1' },
    @{ Series = '0.4'; Version = '0.1.0-snapshot.2' },
    @{ Series = '0.4'; Version = '0.1.0-snapshot.10' },
    @{ Series = '0.4'; Version = '0.1.0-snapshot.65534' },
    @{ Series = '0.4'; Version = '0.1.0' },
    @{ Series = '0.5'; Version = '0.1.1-snapshot.1' },
    @{ Series = '0.5'; Version = '0.1.1' },
    @{ Series = '0.6'; Version = '0.1.2-snapshot.1' },
    @{ Series = '0.6'; Version = '0.1.2-snapshot.2' },
    @{ Series = '0.6'; Version = '0.1.2-snapshot.3' },
    @{ Series = '0.6'; Version = '0.1.2-snapshot.4' },
    @{ Series = '0.6'; Version = '0.1.2' }
)) {
    $current = [version](& "$PSScriptRoot/product-version.ps1" @case)
    if ($current -le $previous) { throw "Upgrade ordering failed for $($case.Version)" }
    $previous = $current
}
foreach ($invalid in '0.1.0-snapshot', '0.1.0-snapshot.0', '0.1.0-snapshot.65535', '0.1.0-beta.1') {
    try { & "$PSScriptRoot/product-version.ps1" -Series '0.4' -Version $invalid }
    catch { continue }
    throw "Unexpectedly accepted '$invalid'"
}
