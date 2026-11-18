//! Windows endpoint discovery shared by the engine and settings window.
//!
//! Persist the value returned by [`cpal::Device::id`](cpal::traits::DeviceTrait::id)
//! rather than an index into this list. Windows may reorder endpoints after a reconnect.

use cpal::{Device, traits::DeviceTrait};
use orvia_core::ADAPTER;

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

/// Identify compatible adapters by their interface property or Windows friendly
/// name suffix. A user-renamed endpoint may still expose the original adapter.
/// Unknown endpoints are never used for playback.
pub fn is_orvia(device: &Device) -> bool {
    device.description().is_ok_and(|desc| {
        let suffix = format!("({ADAPTER})");
        desc.driver() == Some(ADAPTER)
            || desc.name() == ADAPTER
            || desc.extended().iter().any(|s| s.ends_with(&suffix))
    })
}

/// List the stable ID and display name of each selectable physical input.
///
/// # Errors
///
/// Returns a host enumeration error. Devices removed during enumeration are skipped.
pub fn inputs() -> Result<Vec<(String, String)>, cpal::DevicesError> {
    use cpal::traits::HostTrait;
    Ok(cpal::default_host()
        .input_devices()?
        .filter(|d| !is_orvia(d))
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}
