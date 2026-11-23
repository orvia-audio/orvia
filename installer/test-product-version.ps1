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
    @{ Series = '0.6'; Version = '0.1.2' },
    @{ Series = '0.7'; Version = '0.1.3-snapshot.1' },
    @{ Series = '0.7'; Version = '0.1.3-snapshot.2' },
    @{ Series = '0.7'; Version = '0.1.3-snapshot.3' },
    @{ Series = '0.7'; Version = '0.1.3-snapshot.4' },
    @{ Series = '0.7'; Version = '0.1.3-snapshot.5' },
    @{ Series = '0.7'; Version = '0.1.3' },
    @{ Series = '0.8'; Version = '0.1.4' },
    @{ Series = '0.9'; Version = '0.1.5-snapshot.1' },
    @{ Series = '0.9'; Version = '0.1.5' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.1' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.2' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.3' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.4' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.5' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.6' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.7' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.8' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.9' },
    @{ Series = '0.10'; Version = '0.1.6-snapshot.10' },
    @{ Series = '0.10'; Version = '0.1.6' }
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
