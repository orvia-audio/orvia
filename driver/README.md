# Orvia Audio

The Windows 11 x64 test driver exposes a render endpoint for the engine and a capture endpoint for Windows recording clients. Transport v1 uses stereo 48 kHz signed 16-bit PCM. The two WaveRT pins share a bounded, nonpaged queue. Capture emits silence on underflow, and the driver flushes old data across stream transitions. Windows shared mode lets the audio service fan capture out to multiple clients. The render endpoint is excluded from default playback selection.

The PortCls and WaveRT code derives from Microsoft's [Simple Audio Sample at d49bb7d](https://github.com/microsoft/Windows-driver-samples/tree/d49bb7d72f3651952b2b6e4ad28d14fe2247eb69/audio/simpleaudiosample). This directory retains the [Microsoft Public License](LICENSE). The solution uses the sample's project names so changes can be compared with that revision.

## Build a test package

Use Windows 11 x64, Visual Studio 2022 C++ Build Tools, a Windows SDK and NuGet CLI. From the repository root in an **x64 Native Tools Command Prompt** run the following commands.

~~~bat
nuget install Microsoft.Windows.WDK.x64 -Version 10.0.26100.6584 -OutputDirectory target\wdk -Source https://api.nuget.org/v3/index.json -NonInteractive
powershell -NoProfile -File driver\build.ps1 -TestSign
~~~

The script compiles the driver, signs the SYS, validates the INF, builds its catalog and signs the CAT. The package is in `target/driver/package`. The adjacent `Orvia-Test.cer` contains the public certificate. The script discards its temporary signing key. Each signed build has a different certificate. Omit `-TestSign` for a compile and catalog check.

## Load the driver in a lab

Use a disposable Windows 11 x64 test machine configured to allow test-signed kernel drivers. Prepare it according to Microsoft's instructions, then import the public certificate from the **same build** into Local Machine Trusted Root Certification Authorities and Trusted Publishers on that test machine. Run the matching `Orvia-<version>-windows-x64-test.msi` from `dist/lab`. It registers one device with instance ID `ROOT\ORVIAAUDIO\0000`. Windows Installed apps removes it when the lab edition is uninstalled. Remove the test certificate from the lab stores after testing.

Standard Secure Boot installations reject this development signature. The installer leaves boot policy and certificate trust under the lab administrator's control. See [test-signing boot policy](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option) and [installing test-signed packages](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/installing-a-test-signed-driver-package-on-the-test-computer).
