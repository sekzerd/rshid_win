//! Device enumeration and the device-level view of a HID device.

use std::collections::HashMap;

use crate::endpoint::HidDeviceEndpoint;
use crate::error::Result;
use crate::winapi::{self, DeviceString};

/// Describes one physical HID device.
#[derive(Debug)]
pub struct HidDevice {
    /// Vendor ID from `HIDD_ATTRIBUTES`.
    pub vendor_id: u16,
    /// Product ID from `HIDD_ATTRIBUTES`.
    pub product_id: u16,

    /// HID descriptor version (`bcdHID`). Drivers that do not expose the HID
    /// descriptor fall back to [`release_number`](Self::release_number).
    pub version_number: u16,

    /// Product string descriptor. Empty when the device does not expose it.
    pub product: String,
    /// Manufacturer string descriptor. Empty when the device does not expose
    /// it.
    pub manufacturer: String,
    /// Serial number string descriptor. Empty when the device does not expose
    /// it.
    pub serial_number: String,

    /// USB interface number (the `mi_XX` part of an interface path), or -1
    /// when the device is not exposed per interface.
    pub interface_number: i32,

    /// Device release number (`bcdDevice`) reported through
    /// `HIDD_ATTRIBUTES`.
    pub release_number: u16,

    /// One entry per top-level collection of the device, in enumeration order.
    ///
    /// Only endpoints with a non-zero
    /// [`output_length`](HidDeviceEndpoint::output_length) accept
    /// [`write_report`](HidDeviceEndpoint::write_report); input-only
    /// collections reject writes.
    pub endpoints: Vec<HidDeviceEndpoint>,
}

/// Returns the present HID devices.
///
/// Collections that cannot be opened are skipped: Windows hands exclusive
/// keyboard and mouse collections to the system, so their report sizes are
/// unknown and they would be useless as endpoints. The device strings
/// (product, manufacturer and serial number) are read from an accessible
/// collection of the device.
///
/// # Errors
///
/// Returns [`Error`](crate::Error) when SetupAPI cannot enumerate the device
/// interfaces. Unreadable collections are skipped rather than reported.
pub fn enumerate() -> Result<Vec<HidDevice>> {
    let interfaces = winapi::enumerate_interfaces()?;

    let mut devices: Vec<HidDevice> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for interface in interfaces {
        // Collections the system owns exclusively (keyboard, mouse) cannot be
        // opened; they would have no usable report sizes either.
        let Ok(handle) = winapi::create_file(&interface.path) else {
            continue;
        };

        let attributes = winapi::attributes(handle);
        let caps = winapi::caps(handle);
        let manufacturer = winapi::device_string(handle, DeviceString::Manufacturer);
        let product = winapi::device_string(handle, DeviceString::Product);
        let serial_number = winapi::device_string(handle, DeviceString::SerialNumber);
        let mut version = winapi::hid_version(handle);
        let _ = winapi::close_handle(handle);

        let (attributes, caps) = match (attributes, caps) {
            (Ok(attributes), Ok(caps)) => (attributes, caps),
            _ => continue,
        };
        if version == 0 {
            version = attributes.VersionNumber;
        }

        let interface_number = winapi::interface_number(&interface.path);
        let endpoint = HidDeviceEndpoint::from_caps(interface.path, &caps);

        // Collections of one device share a device ID.
        let key = if interface.device_id.is_empty() {
            format!(
                "{:04X}:{:04X}:{}",
                attributes.VendorID, attributes.ProductID, interface_number
            )
        } else {
            interface.device_id
        };

        if let Some(&position) = index.get(&key) {
            devices[position].endpoints.push(endpoint);
            continue;
        }

        index.insert(key, devices.len());
        devices.push(HidDevice {
            vendor_id: attributes.VendorID,
            product_id: attributes.ProductID,
            version_number: version,
            product,
            manufacturer,
            serial_number,
            interface_number,
            release_number: attributes.VersionNumber,
            endpoints: vec![endpoint],
        });
    }

    Ok(devices)
}
