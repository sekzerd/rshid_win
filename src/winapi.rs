//! Raw Win32 plumbing: SetupAPI enumeration, HID queries and file handles.
//!
//! Everything in this module is a thin, mostly mechanical wrapper around the
//! `setupapi.dll`, `hid.dll` and `cfgmgr32.dll` entry points that `hidapi`
//! uses internally as well.

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Parent, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    GUID_DEVINTERFACE_HID, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData,
    HidD_GetAttributes, HidD_GetManufacturerString, HidD_GetPreparsedData, HidD_GetProductString,
    HidD_GetSerialNumberString, HidP_GetCaps,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_ITEMS, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::error::{Error, Result};

/// `IOCTL_HID_GET_DEVICE_DESCRIPTOR` is
/// `CTL_CODE(FILE_DEVICE_KEYBOARD, 0x00, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
const IOCTL_HID_GET_DEVICE_DESCRIPTOR: u32 = 0x000B_0000;

/// Size Windows expects in `SP_DEVICE_INTERFACE_DETAIL_DATA_W::cbSize`.
///
/// The struct is packed on 32-bit builds and naturally aligned on 64-bit ones,
/// which makes the documented `sizeof` 5 and 8 respectively.
const DETAIL_DATA_CB_SIZE_32: u32 = 5;
const DETAIL_DATA_CB_SIZE_64: u32 = 8;

/// A string descriptor that can be read from an open collection.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DeviceString {
    Manufacturer,
    Product,
    SerialNumber,
}

/// One HID top-level collection interface reported by SetupAPI.
#[derive(Debug, Clone)]
pub(crate) struct DeviceInterface {
    /// Device interface path used to open the collection.
    pub(crate) path: String,
    /// Identifies the physical device: the parent devnode ID, which all
    /// collections of one device share. See [`parent_device_id`].
    pub(crate) device_id: String,
}

/// Lists every present HID device interface.
pub(crate) fn enumerate_interfaces() -> Result<Vec<DeviceInterface>> {
    let guid = GUID_DEVINTERFACE_HID;
    let set = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == INVALID_HANDLE_VALUE as isize {
        return Err(Error::last_os("SetupDiGetClassDevsW"));
    }

    // Destroy the device information set even when enumeration fails.
    let result = enumerate_interfaces_with(set, &guid);
    unsafe { SetupDiDestroyDeviceInfoList(set) };
    result
}

fn enumerate_interfaces_with(
    set: HDEVINFO,
    guid: &windows_sys::core::GUID,
) -> Result<Vec<DeviceInterface>> {
    let mut interfaces = Vec::new();

    for index in 0u32.. {
        let mut class_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };

        let ok = unsafe { SetupDiEnumDeviceInterfaces(set, null(), guid, index, &mut class_data) };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                return Ok(interfaces);
            }
            return Err(Error::win32("SetupDiEnumDeviceInterfaces", error));
        }

        let (path, info) = interface_detail(set, &class_data)?;

        // Collections of one physical device are separate devnodes with
        // distinct instance IDs, so group them by their shared parent devnode
        // instead.
        let mut device_id = parent_device_id(&info);
        if device_id.is_empty() {
            device_id = device_instance_id(set, &info);
        }

        interfaces.push(DeviceInterface { path, device_id });
    }

    Ok(interfaces)
}

/// Returns the device path of an enumerated interface together with the device
/// information element it belongs to, which is what groups the collections of
/// one physical device.
fn interface_detail(
    set: HDEVINFO,
    class_data: &SP_DEVICE_INTERFACE_DATA,
) -> Result<(String, SP_DEVINFO_DATA)> {
    let mut info = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let mut required = 0u32;
    // First call: with a null detail buffer the API reports the required size
    // and fails with ERROR_INSUFFICIENT_BUFFER, which is expected.
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(set, class_data, null_mut(), 0, &mut required, null_mut());
    }
    if required == 0 {
        return Err(Error::last_os("SetupDiGetDeviceInterfaceDetailW"));
    }

    // The detail data starts with a u32 and is followed by the UTF-16 path, so
    // a Vec<u32> provides the 4-byte alignment the struct requires.
    let mut buffer = vec![0u32; (required as usize).div_ceil(size_of::<u32>())];
    buffer[0] = if size_of::<usize>() == size_of::<u32>() {
        DETAIL_DATA_CB_SIZE_32
    } else {
        DETAIL_DATA_CB_SIZE_64
    };

    let mut size = required;
    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            class_data,
            buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>(),
            required,
            &mut size,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(Error::last_os("SetupDiGetDeviceInterfaceDetailW"));
    }

    let path = utf16_path(&buffer);
    if path.is_empty() {
        return Err(Error::InvalidData(
            "rshid_win: device interface path is empty",
        ));
    }

    Ok((path, info))
}

/// Decodes the UTF-16 device path that follows the leading `cbSize` field.
fn utf16_path(buffer: &[u32]) -> String {
    let capacity = buffer.len() * 2; // number of u16 code units
    let base = buffer.as_ptr().cast::<u16>();

    let mut length = 0usize;
    while 2 + length < capacity {
        let unit = unsafe { *base.add(2 + length) };
        if unit == 0 {
            break;
        }
        length += 1;
    }

    let units = unsafe { std::slice::from_raw_parts(base.add(2), length) };
    String::from_utf16_lossy(units)
}

/// Returns the device instance ID shared by all collections of one device,
/// e.g. `HID\VID_1234&PID_4321&MI_00\8&2d84e3db&0&0000`.
///
/// The result is empty when SetupAPI cannot provide one.
fn device_instance_id(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> String {
    let mut buffer = [0u16; 256];
    let mut required = 0u32;

    let ok = unsafe {
        SetupDiGetDeviceInstanceIdW(
            set,
            info,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut required,
        )
    };
    if ok == 0 {
        return String::new();
    }
    utf16_to_string(&buffer)
}

/// Returns the device ID of the parent devnode of an enumerated collection.
///
/// Each top-level collection is a devnode of its own, but they all hang off the
/// same interface devnode, so this is what groups the collections of one
/// physical device. The result is empty when unavailable.
fn parent_device_id(info: &SP_DEVINFO_DATA) -> String {
    if info.DevInst == 0 {
        return String::new();
    }

    let mut parent = 0u32;
    if unsafe { CM_Get_Parent(&mut parent, info.DevInst, 0) } != 0 {
        return String::new();
    }

    let mut buffer = [0u16; 256];
    if unsafe { CM_Get_Device_IDW(parent, buffer.as_mut_ptr(), buffer.len() as u32, 0) } != 0 {
        return String::new();
    }
    utf16_to_string(&buffer)
}

/// Opens a HID collection for reading and writing.
pub(crate) fn create_file(path: &str) -> Result<HANDLE> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::last_os("CreateFileW"));
    }
    Ok(handle)
}

/// Releases a handle obtained from [`create_file`].
pub(crate) fn close_handle(handle: HANDLE) -> Result<()> {
    if unsafe { CloseHandle(handle) } == 0 {
        return Err(Error::last_os("CloseHandle"));
    }
    Ok(())
}

/// Queries vendor ID, product ID and version of an open collection.
pub(crate) fn attributes(handle: HANDLE) -> Result<HIDD_ATTRIBUTES> {
    let mut attributes = HIDD_ATTRIBUTES {
        Size: size_of::<HIDD_ATTRIBUTES>() as u32,
        VendorID: 0,
        ProductID: 0,
        VersionNumber: 0,
    };

    if !unsafe { HidD_GetAttributes(handle, &mut attributes) } {
        return Err(Error::last_os("HidD_GetAttributes"));
    }
    Ok(attributes)
}

/// Queries the report sizes of an open collection.
pub(crate) fn caps(handle: HANDLE) -> Result<HIDP_CAPS> {
    let mut preparsed: isize = 0;
    if !unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } || preparsed == 0 {
        return Err(Error::last_os("HidD_GetPreparsedData"));
    }

    let mut caps: HIDP_CAPS = unsafe { zeroed() };
    let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };

    let _ = unsafe { HidD_FreePreparsedData(preparsed) };

    if status != HIDP_STATUS_SUCCESS {
        return Err(Error::win32(
            "HidP_GetCaps",
            io::Error::from_raw_os_error(status),
        ));
    }
    Ok(caps)
}

/// Reads a string descriptor from an open collection.
///
/// The value is empty when the device does not provide it.
pub(crate) fn device_string(handle: HANDLE, which: DeviceString) -> String {
    let mut buffer = [0u16; 128];
    let length = (buffer.len() * size_of::<u16>()) as u32;
    let pointer = buffer.as_mut_ptr().cast::<c_void>();

    let ok = unsafe {
        match which {
            DeviceString::Manufacturer => HidD_GetManufacturerString(handle, pointer, length),
            DeviceString::Product => HidD_GetProductString(handle, pointer, length),
            DeviceString::SerialNumber => HidD_GetSerialNumberString(handle, pointer, length),
        }
    };
    if !ok {
        return String::new();
    }
    utf16_to_string(&buffer)
}

/// Returns `bcdHID` from the HID descriptor, or 0 when it is unavailable.
pub(crate) fn hid_version(handle: HANDLE) -> u16 {
    // HID_DESCRIPTOR is 9 packed bytes; keep it as a byte slice so alignment
    // cannot shift the fields.
    let mut descriptor = [0u8; 9];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_HID_GET_DEVICE_DESCRIPTOR,
            null(),
            0,
            descriptor.as_mut_ptr().cast::<c_void>(),
            descriptor.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 || returned < 4 {
        return 0;
    }
    u16::from(descriptor[2]) | (u16::from(descriptor[3]) << 8)
}

/// Extracts the USB interface number from a device path such as
/// `\\?\hid#vid_1234&pid_4321&mi_00&col03#8&2d84e3db&0&0002#{...}`.
///
/// Returns -1 for devices that are not exposed per USB interface.
pub(crate) fn interface_number(path: &str) -> i32 {
    let bytes = path.as_bytes();
    let Some(index) = bytes
        .windows(4)
        .position(|window| window.eq_ignore_ascii_case(b"&mi_"))
    else {
        return -1;
    };
    if bytes.len() < index + 6 {
        return -1;
    }

    // Device paths are ASCII, so these two bytes always form a valid str.
    let digits = &bytes[index + 4..index + 6];
    let Ok(digits) = std::str::from_utf8(digits) else {
        return -1;
    };
    match u8::from_str_radix(digits, 16) {
        Ok(number) => i32::from(number),
        Err(_) => -1,
    }
}

/// Decodes a NUL-terminated UTF-16 buffer.
fn utf16_to_string(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_number_from_usb_path() {
        let path = r"\\?\hid#vid_1234&pid_4321&mi_02&col03#8&2d84e3db&0&0002#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_number(path), 2);
    }

    #[test]
    fn interface_number_without_mi_part() {
        let path =
            r"\\?\hid#vid_1234&pid_4321#8&2d84e3db&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_number(path), -1);
    }

    #[test]
    fn interface_number_is_case_insensitive() {
        let path = r"\\?\HID#VID_1234&PID_4321&MI_0A&COL01#8&2d84e3db&0&0000";
        assert_eq!(interface_number(path), 10);
    }

    #[test]
    fn interface_number_rejects_short_paths() {
        assert_eq!(interface_number("&mi_"), -1);
        assert_eq!(interface_number("&mi_0"), -1);
    }

    #[test]
    fn utf16_string_stops_at_nul() {
        let buffer = [b'H' as u16, b'I' as u16, b'D' as u16, 0, b'X' as u16];
        assert_eq!(utf16_to_string(&buffer), "HID");
    }
}
