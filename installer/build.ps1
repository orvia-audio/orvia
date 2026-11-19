# Run from an x64 Native Tools shell. The lab build also needs a test-signed driver.
[CmdletBinding()]
param([switch]$WithTestDriver)
$ErrorActionPreference = 'Stop'
Import-Module "$PSHOME/Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1"
Import-Module "$PSHOME/Modules/Microsoft.PowerShell.Utility/Microsoft.PowerShell.Utility.psd1"
$root = Split-Path $PSScriptRoot
$work = New-Item -Type Directory -Force "$root/target/installer"
$destination = if ($WithTestDriver) { "$root/dist/lab" } else { "$root/dist" }
$dist = New-Item -Type Directory -Force $destination
$driver = "$root/target/driver/package"
$cer = "$root/target/driver/Orvia-Test.cer"
function Run([string]$Tool, [string[]]$Arguments) {
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Tool exited with $LASTEXITCODE" }
}
Push-Location $root
try {
    $driverFlags = @()
    if ($WithTestDriver) {
        $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 $cer
        foreach ($file in 'OrviaAudio.sys','OrviaAudio.cat') {
            $signature = Get-AuthenticodeSignature -LiteralPath "$driver/$file"
            if ($signature.SignerCertificate.Thumbprint -ne $cert.Thumbprint) {
                throw "Build the SYS and CAT with driver/build.ps1 -TestSign before packaging."
            }
        }
        $driverFlags = @('-d','LabDriver=1','-d',"Driver=$driver")
        Run cl.exe @('/nologo','/std:c++17','/O2','/EHsc','/MT','/DUNICODE','/D_UNICODE',
            "/Fo$work/device.obj","/Fe$work/orvia-device.exe","$PSScriptRoot/device.cpp",
            '/link','setupapi.lib','newdev.lib','cfgmgr32.lib')
    }
    $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $app = $metadata.packages | Where-Object name -eq 'orvia'
    $version = $app.version
    $productVersion = & "$PSScriptRoot/product-version.ps1" -Version $version -Series $metadata.metadata.'windows-installer'.series
    $appBuild = cargo build --workspace --locked --release --message-format=json-render-diagnostics |
        ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.reason -eq 'build-script-executed' -and $_.package_id -eq $app.id }
    if ($LASTEXITCODE -ne 0 -or !$appBuild) { throw "Application build did not produce icon resources" }
    $icon = Join-Path $appBuild.out_dir 'orvia.ico'
    Run cargo-about @('generate','--locked','-o',"$work/dependencies.txt","$PSScriptRoot/notices.hbs")
    $notices = @("Orvia $version", (Get-Content LICENSE -Raw),
        'Tabler Icons', (Get-Content "$root/assets/tabler/LICENSE" -Raw -Encoding UTF8),
        'Microsoft Visual C++ Runtime, Copyright Microsoft Corporation. Distributed under the Visual Studio redistribution terms.',
        (Get-Content "$work/dependencies.txt" -Raw -Encoding UTF8))
    if ($WithTestDriver) { $notices += 'Orvia Audio driver', (Get-Content driver/LICENSE -Raw) }
    [IO.File]::WriteAllText("$dist/THIRD-PARTY-NOTICES.txt", ($notices -join [Environment]::NewLine))
    $suffix = if ($WithTestDriver) { '-test' } else { '' }
    $name = "Orvia-$version-windows-x64$suffix.msi"
    $runtime = Get-Item "$env:VCToolsRedistDir/x64/Microsoft.VC*.CRT/vcruntime140.dll" | Select-Object -First 1
    Run wix (@('build','-arch','x64','-d',"Runtime=$($runtime.FullName)",'-d',"AppVersion=$version",'-d',"ProductVersion=$productVersion",'-d',"Work=$work",'-d',"Dist=$dist",
        '-d',"Bin=$root/target/release",'-d',"Icon=$icon",'-intermediateFolder',"$work/wix$suffix",
        '-pdbtype','none','-out',"$dist/$name","$PSScriptRoot/Orvia.wxs") + $driverFlags)
    $files = @($name,'THIRD-PARTY-NOTICES.txt')
    if ($WithTestDriver) {
        Copy-Item -LiteralPath $cer -Destination "$dist/Orvia-Test.cer" -Force
        $files += 'Orvia-Test.cer'
    }
    $lines = foreach ($item in $files) {
        (Get-FileHash "$dist/$item" -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $item
    }
    [IO.File]::WriteAllLines("$dist/SHA256SUMS.txt", [string[]]$lines)
} finally { Pop-Location }
