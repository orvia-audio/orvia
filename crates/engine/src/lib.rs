//! Windows endpoint discovery shared by the engine and settings window.
//!
//! Persist the value returned by [`cpal::Device::id`](cpal::traits::DeviceTrait::id)
//! rather than an index into this list. Windows may reorder endpoints after a reconnect.

use cpal::{
    Device, DeviceDescription,
    traits::{DeviceTrait, HostTrait},
};
use orvia_core::ADAPTER;

const VAC: &str = "Virtual Audio Cable";
const VIRTUAL_ADAPTERS: [&str; 3] = [ADAPTER, VAC, "Elgato Virtual Audio"];

// WASAPI normally exposes the adapter separately from the localized endpoint
// name. Some endpoints expose it only in the full Windows friendly name.
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
        matches(desc.name()) || desc.extended().iter().any(|s| matches(s))
    })
}

/// Return the full friendly name, including the adapter name when available.
///
/// # Errors
///
/// Propagates failure to read device properties, for example during removal.
pub fn label(device: &Device) -> Result<String, cpal::DeviceNameError> {
    let desc = device.description()?;
    Ok(desc
        .extended()
        .first()
        .map(String::as_str)
        .unwrap_or(desc.name())
        .to_owned())
}

/// Identify the built-in lab adapter used when an older configuration has no output ID.
pub fn is_orvia(device: &Device) -> bool {
    device
        .description()
        .is_ok_and(|desc| adapter(&desc) == Some(ADAPTER))
}

/// Recognize the recording and playback sides of supported virtual cables.
/// Windows enforces driver signing when it loads the installed driver.
pub fn is_virtual_cable(device: &Device) -> bool {
    device
        .description()
        .is_ok_and(|desc| adapter(&desc).is_some())
}

/// List the stable ID and display name of each selectable physical input.
///
/// # Errors
///
/// Returns a host enumeration error. Devices removed during enumeration are skipped.
pub fn inputs() -> Result<Vec<(String, String)>, cpal::DevicesError> {
    Ok(cpal::default_host()
        .input_devices()?
        .filter(|d| !is_virtual_cable(d))
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}

/// List stable playback IDs and names for VAC, Elgato Virtual Audio and Orvia Audio v1.
/// A recording app opens the corresponding capture endpoint independently.
///
/// # Errors
///
/// Returns a host enumeration error. Devices removed during enumeration are skipped.
pub fn outputs() -> Result<Vec<(String, String)>, cpal::DevicesError> {
    Ok(cpal::default_host()
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
pub fn monitors() -> Result<Vec<(String, String)>, cpal::DevicesError> {
    Ok(cpal::default_host()
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
