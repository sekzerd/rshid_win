//! `rshid_win` is a small USB HID library for Windows.
//!
//! It talks to the Windows HID stack directly (`setupapi.dll`, `hid.dll` and
//! `cfgmgr32.dll`), the same APIs `hidapi` uses internally, so it needs
//! neither a C toolchain nor a driver installation. The behaviour mirrors the
//! `gohid_win` package.
//!
//! A physical device usually exposes several top-level collections (for
//! example a keyboard reports keys, consumer controls and vendor data through
//! separate collections). [`enumerate`] groups those collections per device:
//! every [`HidDevice`] carries the device level identity (including the string
//! descriptors such as the serial number) and a list of
//! [`HidDeviceEndpoint`], one per collection. Only endpoints whose
//! [`output_length`](HidDeviceEndpoint::output_length) is not zero accept
//! writes.
//!
//! HID reports are addressed by a report ID that is the first byte of the
//! report. Reports passed to [`write_report`](HidDeviceEndpoint::write_report),
//! [`write_feature_report`](HidDeviceEndpoint::write_feature_report) and
//! [`read_feature_report`](HidDeviceEndpoint::read_feature_report) therefore
//! must start with that ID; the library pads them to the endpoint's report
//! length, as Windows rejects buffers of any other size.
//!
//! # Example
//!
//! ```no_run
//! fn main() -> Result<(), rshid_win::Error> {
//!     let mut devices = rshid_win::enumerate()?;
//!
//!     for device in &mut devices {
//!         if device.vendor_id != 0x1234 || device.product_id != 0x4321 {
//!             continue;
//!         }
//!         for endpoint in &mut device.endpoints {
//!             if endpoint.output_length == 0 {
//!                 continue; // input-only collection, writes are rejected
//!             }
//!             endpoint.open()?;
//!             endpoint.write_report(&[0x00, 0x55, 0x65])?;
//!             if endpoint.input_length != 0 {
//!                 let reply = endpoint.read_report()?;
//!                 println!("{reply:02X?}");
//!             }
//!             endpoint.close()?;
//!             return Ok(());
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

#[cfg(not(windows))]
compile_error!("rshid_win only supports Windows");

#[cfg(windows)]
mod device;
#[cfg(windows)]
mod endpoint;
#[cfg(windows)]
mod error;
#[cfg(windows)]
mod winapi;

#[cfg(windows)]
pub use device::{HidDevice, enumerate};
#[cfg(windows)]
pub use endpoint::HidDeviceEndpoint;
#[cfg(windows)]
pub use error::{Error, ReportKind, Result};
