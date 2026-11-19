//! Audio endpoint discovery shared by the engine and settings window.
//!
//! Persist the value returned by [`cpal::Device::id`](cpal::traits::DeviceTrait::id)
//! rather than an index into this list. A host may reorder endpoints after a reconnect.

#[cfg(any(windows, test))]
use cpal::DeviceDescription;
use cpal::{
    Device,
    traits::{DeviceTrait, HostTrait},
};
#[cfg(any(windows, test))]
use orvia_core::ADAPTER;

#[cfg(any(windows, test))]
const VAC: &str = "Virtual Audio Cable";
#[cfg(any(windows, test))]
const VIRTUAL_ADAPTERS: [&str; 3] = [ADAPTER, VAC, "Elgato Virtual Audio"];

// WASAPI normally exposes the adapter separately from the localized endpoint
// name. Some endpoints expose it only in the full Windows friendly name.
#[cfg(any(windows, test))]
fn adapter(desc: &DeviceDescription) -> Option<&'static str> {
    if let Some(driver) = desc.driver() {
        return VIRTUAL_ADAPTERS.into_iter().find(|&name| name == driver);
    }
    VIRTUAL_ADAPTERS.into_iter().find(|&name| {
        let matches = |label: &str| {
            label == name
                || label
                    .strip_suffix(')')
                    .and_then(|s| s.rsplit_once(" ("))
                    .is_some_and(|(_, suffix)| suffix == name)
        };
        matches(desc.name()) || desc.extended().any(matches)
    })
}

/// Return the full friendly name, including the adapter name when available.
///
/// # Errors
///
/// Propagates failure to read device properties, for example during removal.
pub fn label(device: &Device) -> Result<String, cpal::Error> {
    let desc = device.description()?;
    Ok(desc.extended().next().unwrap_or(desc.name()).to_owned())
}

/// Open the desktop sound server used for both enumeration and streaming.
///
/// # Errors
///
/// On Linux, returns an error when the PulseAudio-compatible server is unavailable.
pub fn host() -> Result<cpal::Host, cpal::Error> {
    #[cfg(target_os = "linux")]
    {
        cpal::host_from_id(cpal::HostId::PulseAudio)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(cpal::default_host())
    }
}

/// Identify the lab adapter used by Windows installations without an explicit output.
#[cfg(windows)]
pub fn is_orvia(device: &Device) -> bool {
    device
        .description()
        .is_ok_and(|desc| adapter(&desc) == Some(ADAPTER))
}

/// Recognize supported virtual devices and exclude their inputs from microphone capture.
pub fn is_virtual_cable(device: &Device) -> bool {
    #[cfg(windows)]
    {
        device
            .description()
            .is_ok_and(|desc| adapter(&desc).is_some())
    }
    #[cfg(target_os = "macos")]
    {
        device.id().is_ok_and(|id| id.id() == "BlackHole2ch_UID")
    }
    #[cfg(target_os = "linux")]
    {
        device.id().is_ok_and(|id| {
            matches!(id.id(), "orvia_output" | "orvia_microphone") || id.id().ends_with(".monitor")
        })
    }
}

/// List the stable ID and display name of each selectable physical input.
///
/// # Errors
///
/// Returns a host enumeration error. Devices removed during enumeration are skipped.
pub fn inputs() -> Result<Vec<(String, String)>, cpal::Error> {
    Ok(host()?
        .input_devices()?
        .filter(|d| !is_virtual_cable(d))
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}

/// List stable playback IDs and names for supported virtual microphones.
/// A recording app opens the corresponding capture endpoint independently.
///
/// # Errors
///
/// Returns a host enumeration error. Devices removed during enumeration are skipped.
pub fn outputs() -> Result<Vec<(String, String)>, cpal::Error> {
    Ok(host()?
        .output_devices()?
        .filter(is_virtual_cable)
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}

/// List playback devices available for headphone monitoring. Known virtual
/// cables are excluded so monitoring cannot feed the processed route back into itself.
///
/// # Errors
///
/// Returns a host enumeration error. Endpoints removed during discovery are skipped.
pub fn monitors() -> Result<Vec<(String, String)>, cpal::Error> {
    Ok(host()?
        .output_devices()?
        .filter(|d| !is_virtual_cable(d))
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::DeviceDescriptionBuilder;

    #[test]
    fn identifies_the_adapter_across_localized_and_renamed_endpoints() {
        let renamed = DeviceDescriptionBuilder::new("Recording cable")
            .driver(VAC)
            .build();
        let localized = DeviceDescriptionBuilder::new("ライン 1")
            .add_extended_line("ライン 1 (Virtual Audio Cable)")
            .build();
        for desc in [renamed, localized] {
            assert_eq!(adapter(&desc), Some(VAC));
        }
        let wave_link = DeviceDescriptionBuilder::new("Wave Link Aux 1")
            .driver("Elgato Virtual Audio")
            .build();
        assert_eq!(adapter(&wave_link), Some("Elgato Virtual Audio"));
        let speakers = DeviceDescriptionBuilder::new("Line 1 (Virtual Audio Cable)")
            .driver("Steinberg UR12")
            .build();
        assert_eq!(adapter(&speakers), None);
        assert_eq!(
            adapter(&DeviceDescriptionBuilder::new("Playback (Orvia Audio v1)").build()),
            Some(ADAPTER)
        );
        for name in [
            "Speakers (Steinberg UR12)",
            "Line 1",
            "Virtual Audio Cable Speakers",
            "Headphones",
        ] {
            assert_eq!(adapter(&DeviceDescriptionBuilder::new(name).build()), None);
        }
    }
}
