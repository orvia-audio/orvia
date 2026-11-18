// Windows Installer invokes this helper from its protected installation directory.
// A fixed instance ID lets app-only upgrades keep callers' microphone selection.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <setupapi.h>
#include <newdev.h>
#include <cfgmgr32.h>
#include <initguid.h>
#include <devguid.h>
#include <devpkey.h>
#include <filesystem>
#include <iostream>
#include <string>

namespace {
constexpr wchar_t Instance[] = L"ROOT\\ORVIAAUDIO\\0000";
constexpr wchar_t Hardware[] = L"ROOT\\OrviaAudio\0"; // REG_MULTI_SZ

struct Devices {
    HDEVINFO handle = SetupDiCreateDeviceInfoList(&GUID_DEVCLASS_MEDIA, nullptr);
    ~Devices() { if (handle != INVALID_HANDLE_VALUE) SetupDiDestroyDeviceInfoList(handle); }
};

std::wstring installedInf(HDEVINFO set, SP_DEVINFO_DATA& device) {
    wchar_t name[MAX_PATH] = {};
    DEVPROPTYPE type;
    if (!SetupDiGetDevicePropertyW(set, &device, &DEVPKEY_Device_DriverInfPath, &type,
        reinterpret_cast<PBYTE>(name), sizeof(name), nullptr, 0) || type != DEVPROP_TYPE_STRING) {
        return {};
    }
    return name;
}

DWORD manage(bool install, const std::filesystem::path& inf) {
    Devices devices;
    if (devices.handle == INVALID_HANDLE_VALUE) return GetLastError();
    SP_DEVINFO_DATA device = {sizeof(device)};
    const BOOL exists = SetupDiOpenDeviceInfoW(devices.handle, Instance, nullptr, 0, &device);
    if (!exists && GetLastError() != ERROR_NO_SUCH_DEVINST) return GetLastError();
    const auto previous = exists ? installedInf(devices.handle, device) : L"";
    if (!install) {
        if (!exists) return ERROR_SUCCESS;
        BOOL reboot = FALSE;
        if (!DiUninstallDevice(nullptr, devices.handle, &device, 0, &reboot)) return GetLastError();
        if (!previous.empty() && !SetupUninstallOEMInfW(previous.c_str(), 0, nullptr)) {
            DWORD err = GetLastError();
            if (err != ERROR_FILE_NOT_FOUND) return err;
        }
        return ERROR_SUCCESS;
    }
    if (!exists) {
        if (!SetupDiCreateDeviceInfoW(devices.handle, Instance, &GUID_DEVCLASS_MEDIA,
                L"Orvia Audio v1", nullptr, 0, &device) ||
            !SetupDiSetDeviceRegistryPropertyW(devices.handle, &device, SPDRP_HARDWAREID,
                reinterpret_cast<const BYTE*>(Hardware), sizeof(Hardware)) ||
            !SetupDiCallClassInstaller(DIF_REGISTERDEVICE, devices.handle, &device)) return GetLastError();
    }
    BOOL reboot = FALSE;
    if (!UpdateDriverForPlugAndPlayDevicesW(nullptr, Hardware, inf.c_str(), INSTALLFLAG_NONINTERACTIVE, &reboot)) {
        const DWORD err = GetLastError();
        // SetupAPI may already have selected the same or a newer package.
        if (err == ERROR_NO_MORE_ITEMS && exists && !previous.empty()) return ERROR_SUCCESS;
        if (!exists) DiUninstallDevice(nullptr, devices.handle, &device, 0, &reboot);
        return err;
    }
    const auto current = installedInf(devices.handle, device);
    if (!previous.empty() && !current.empty() && previous != current) {
        SetupUninstallOEMInfW(previous.c_str(), 0, nullptr);
    }
    return ERROR_SUCCESS;
}
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return ERROR_INVALID_PARAMETER;
    const std::wstring action = argv[1];
    if (action != L"install" && action != L"remove") return ERROR_INVALID_PARAMETER;
    wchar_t path[MAX_PATH];
    const DWORD size = GetModuleFileNameW(nullptr, path, MAX_PATH);
    if (!size || size >= MAX_PATH) return ERROR_BAD_PATHNAME;
    const auto result = manage(action == L"install", std::filesystem::path(path).parent_path() / L"OrviaAudio.inf");
    if (result) std::wcerr << L"Device setup failed with Windows error " << result << L'\n';
    return result ? EXIT_FAILURE : EXIT_SUCCESS;
}
