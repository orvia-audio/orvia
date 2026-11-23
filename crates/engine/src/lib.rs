//! Audio endpoint discovery shared by the engine and settings window.
//!
//! Persist the value returned by [`cpal::Device::id`](cpal::traits::DeviceTrait::id)
//! rather than an index into this list. A host may reorder endpoints after a reconnect.

pub mod formats;
pub mod mic_test;

use cpal::{
    Device, DeviceDescription, DeviceId,
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
    Ok(display_name(&desc).to_owned())
}

/// Get the full endpoint label from properties already fetched for validation.
pub fn display_name(desc: &DeviceDescription) -> &str {
    desc.extended().next().unwrap_or(desc.name())
}

/// Identify a cable while reusing the description fetched for this endpoint.
pub fn described_virtual_cable(_device: &Device, _description: &DeviceDescription) -> bool {
    #[cfg(windows)]
    {
        adapter(_description).is_some()
    }
    #[cfg(not(windows))]
    {
        is_virtual_cable(_device)
    }
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
    #[cfg(not(windows))]
    {
        device.id().is_ok_and(|id| virtual_id(id.id()))
    }
}

#[cfg(not(windows))]
fn virtual_id(id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        id == "BlackHole2ch_UID"
    }
    #[cfg(target_os = "linux")]
    {
        matches!(id, "orvia_output" | "orvia_microphone") || id.ends_with(".monitor")
    }
}

/// Device choices for configuring a microphone route, in host enumeration order.
#[derive(Default)]
pub struct RouteDevices {
    /// Physical capture endpoints.
    pub inputs: Vec<(String, String)>,
    /// Playback endpoints that feed supported virtual microphones.
    pub outputs: Vec<(String, String)>,
    /// Virtual capture endpoints available to receiving applications.
    pub receivers: Vec<(String, String)>,
}

impl RouteDevices {
    /// Discover the three lists in one scan using each endpoint's stable ID and description.
    ///
    /// # Errors
    ///
    /// Returns a host enumeration error. Endpoints removed during discovery are skipped.
    pub fn discover() -> Result<Self, cpal::Error> {
        let mut devices = Self::default();
        for device in host()?.devices()? {
            if let Ok(id) = device.id()
                && let Ok(desc) = device.description()
            {
                devices.add(&id, &desc);
            }
        }
        Ok(devices)
    }

    fn add(&mut self, id: &DeviceId, desc: &DeviceDescription) {
        #[cfg(windows)]
        let virtual_cable = adapter(desc).is_some();
        #[cfg(not(windows))]
        let virtual_cable = virtual_id(id.id());
        #[cfg(target_os = "linux")]
        let receiver = mic_test::is_receiver_id(id.id());
        #[cfg(not(target_os = "linux"))]
        let receiver = virtual_cable;

        let entry = || (id.to_string(), display_name(desc).to_owned());
        if desc.supports_input() {
            if !virtual_cable {
                self.inputs.push(entry());
            } else if receiver {
                self.receivers.push(entry());
            }
        }
        if virtual_cable && desc.supports_output() {
            self.outputs.push(entry());
        }
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
    use cpal::{DeviceDescriptionBuilder, DeviceDirection};

    #[test]
    fn routes_physical_inputs_and_both_sides_of_a_duplex_cable_in_host_order() {
        let id = |name: &'static str| DeviceId::new(cpal::ALL_HOSTS[0], name);
        let mic = DeviceDescriptionBuilder::new("Mic")
            .direction(DeviceDirection::Input)
            .build();
        let speaker = DeviceDescriptionBuilder::new("Speakers")
            .direction(DeviceDirection::Output)
            .build();
        let cable = DeviceDescriptionBuilder::new("Cable")
            .driver(VAC)
            .direction(DeviceDirection::Duplex)
            .add_extended_line("Cable pair")
            .build();
        #[cfg(target_os = "macos")]
        let cable_id = id("BlackHole2ch_UID");
        #[cfg(not(target_os = "macos"))]
        let cable_id = id("orvia_microphone");
        let mut devices = RouteDevices::default();
        devices.add(&id("mic-b"), &mic);
        devices.add(&cable_id, &cable);
        devices.add(&id("speaker"), &speaker);
        devices.add(&id("mic-a"), &mic);
        assert_eq!(
            devices.inputs,
            [
                (id("mic-b").to_string(), "Mic".into()),
                (id("mic-a").to_string(), "Mic".into())
            ]
        );
        assert_eq!(
            devices.outputs,
            [(cable_id.to_string(), "Cable pair".into())]
        );
        assert_eq!(devices.receivers, devices.outputs);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_the_packaged_sink_monitor_is_selectable_as_a_virtual_receiver() {
        let mut devices = RouteDevices::default();
        let capture = DeviceDescriptionBuilder::new("Monitor")
            .direction(DeviceDirection::Input)
            .build();
        let host = cpal::HostId::PulseAudio;
        devices.add(&DeviceId::new(host, "speakers.monitor"), &capture);
        let receiver = DeviceId::new(host, "orvia_output.monitor");
        devices.add(&receiver, &capture);
        assert!(devices.inputs.is_empty());
        assert_eq!(
            devices.receivers,
            [(receiver.to_string(), "Monitor".into())]
        );
    }

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
