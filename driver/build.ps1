# Build in an x64 Native Tools Command Prompt for Visual Studio 2022.
[CmdletBinding()]
param([switch]$TestSign)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$kits = Join-Path $root 'target/wdk'
$wdk = Join-Path $kits 'Microsoft.Windows.WDK.x64.10.0.26100.6584/c'
$sdk = Join-Path $kits 'Microsoft.Windows.SDK.CPP.10.0.26100.1/c'
$kitVersion = '10.0.26100.0'
$kmdf = '1.15'
$src = Join-Path $PSScriptRoot 'Source'
$work = New-Item -Type Directory -Force "$root/target/driver"
$package = New-Item -Type Directory -Force "$work/package"
if (!(Test-Path "$wdk/include")) {
    throw 'Restore the pinned Microsoft.Windows.WDK.x64 NuGet package first. See driver/README.md.'
}
function Invoke-BuildTool([string]$Tool, [string[]]$Arguments) {
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Tool exited with $LASTEXITCODE" }
}
$definitions = @('_AMD64_', 'AMD64', '_WIN64', '_WIN32', 'UNICODE', '_UNICODE',
    '_WIN32_WINNT=0x0A00','NTDDI_VERSION=0x0A00000B','PC_IMPLEMENTATION','_USE_WAVERT_',
    '_USE_IPortClsRuntimePower','_NEW_DELETE_OPERATORS_', 'KMDF_VERSION_MAJOR=1', 'KMDF_VERSION_MINOR=15')
$includes = @("$wdk/include/$kitVersion/km", "$wdk/include/$kitVersion/km/crt",
    "$wdk/include/$kitVersion/shared", "$sdk/include/$kitVersion/shared",
    "$wdk/include/wdf/kmdf/$kmdf", "$src/Inc", "$src/Filters", "$src/Main", "$src/Utilities")
Push-Location $work
try {
    $files = Get-ChildItem "$src/Main/*.cpp","$src/Filters/*.cpp","$src/Utilities/*.cpp"
    $compile = @('/nologo','/c','/O2','/kernel','/Zl','/GS','/GR-','/W3','/MP') +
        @($definitions | ForEach-Object { "/D$_" }) + @($includes | ForEach-Object { "/I$_" }) + $files.FullName
    Invoke-BuildTool cl.exe $compile
    $lib = "$wdk/lib/$kitVersion/km/x64"
    $wdf = "$wdk/lib/wdf/kmdf/x64/$kmdf"
    $objects = $files | ForEach-Object { "$($_.BaseName).obj" }
    Invoke-BuildTool link.exe (@('/nologo','/driver','/subsystem:native,10.00','/machine:x64','/nodefaultlib',
        '/entry:FxDriverEntry','/dynamicbase','/nxcompat','/integritycheck','/opt:ref','/opt:icf',
        "/out:$package/OrviaAudio.sys", "/libpath:$lib", "/libpath:$wdf",
        'ntoskrnl.lib','hal.lib','wmilib.lib','portcls.lib','ks.lib','stdunk.lib','libcntpr.lib',
        'BufferOverflowFastFailK.lib','WdfLdr.lib','WdfDriverEntry.lib') + $objects)
    $inf = Get-Content -Raw -Encoding UTF8 "$src/Main/OrviaAudio.inx"
    $inf.Replace('$ARCH$', 'amd64').Replace('$KMDFVERSION$', $kmdf) |
        Set-Content -Encoding Unicode "$package/OrviaAudio.inf"
    $signTool = "$sdk/bin/$kitVersion/x64/signtool.exe"
    if ($TestSign) {
        Import-Module "$PSHOME/Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1"
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=Orvia Development Test' -CertStoreLocation Cert:\CurrentUser\My -KeyExportPolicy NonExportable
        Export-Certificate -Cert $cert -FilePath "$work/Orvia-Test.cer" | Out-Null
        Invoke-BuildTool $signTool @('sign','/fd','SHA256','/sha1',$cert.Thumbprint,"$package/OrviaAudio.sys")
    }
    Invoke-BuildTool "$wdk/tools/$kitVersion/x64/infverif.exe" @('/w',"$package/OrviaAudio.inf")
    Invoke-BuildTool "$wdk/bin/$kitVersion/x86/Inf2Cat.exe" @("/driver:$package",'/os:10_CO_X64','/uselocaltime')
    if ($TestSign) {
        Invoke-BuildTool $signTool @('sign','/fd','SHA256','/sha1',$cert.Thumbprint,"$package/OrviaAudio.cat")
    }
} finally {
    if ($cert) { Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($cert.Thumbprint)" -DeleteKey }
    Pop-Location
}

