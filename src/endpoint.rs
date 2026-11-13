//! Endpoints (top-level collections) and their report I/O.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDP_CAPS, HidD_GetFeature, HidD_SetFeature, HidD_SetNumInputBuffers,
};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};

use crate::error::{Error, ReportKind, Result};
use crate::winapi;

/// A single top-level collection of a [`HidDevice`](crate::HidDevice).
///
/// Endpoints are opened lazily. [`open`](Self::open) is a no-op when the
/// endpoint is already open, and [`close`](Self::close) is a no-op when it is
/// not. Dropping an open endpoint closes its handle as well.
#[derive(Debug)]
pub struct HidDeviceEndpoint {
    /// Device interface path used to open the collection.
    pub path: String,
    /// HID usage of the collection.
    pub usage: u16,
    /// HID usage page of the collection.
    pub usage_page: u16,
    /// Length of an input report, in bytes, including the report ID.
    pub input_length: u16,
    /// Length of an output report, in bytes, including the report ID.
    pub output_length: u16,
    /// Length of a feature report, in bytes, including the report ID.
    pub feature_length: u16,

    handle: Option<HANDLE>,
}

// SAFETY: the handle is only passed to thread-safe Win32 file APIs and is never
// exposed. `Option<HANDLE>` tracks the single owner, so moving the endpoint to
// another thread cannot create a second owner that closes it twice.
unsafe impl Send for HidDeviceEndpoint {}

impl HidDeviceEndpoint {
    pub(crate) fn from_caps(path: String, caps: &HIDP_CAPS) -> Self {
        Self {
            path,
            usage: caps.Usage,
            usage_page: caps.UsagePage,
            input_length: caps.InputReportByteLength,
            output_length: caps.OutputReportByteLength,
            feature_length: caps.FeatureReportByteLength,
            handle: None,
        }
    }

    /// Returns whether the endpoint currently holds an open handle.
    pub fn is_open(&self) -> bool {
        self.handle.is_some()
    }

    /// Opens the endpoint with `CreateFileW`.
    ///
    /// Opening an already open endpoint is a no-op.
    pub fn open(&mut self) -> Result<()> {
        if self.handle.is_some() {
            return Ok(());
        }

        let handle = winapi::create_file(&self.path)?;
        // Best effort: a larger input queue makes `read_report` less likely to
        // lose reports between two reads.
        let _ = unsafe { HidD_SetNumInputBuffers(handle, 64) };
        self.handle = Some(handle);
        Ok(())
    }

    /// Releases the endpoint handle with `CloseHandle`.
    ///
    /// Closing a closed endpoint is a no-op.
    pub fn close(&mut self) -> Result<()> {
        match self.handle.take() {
            Some(handle) => winapi::close_handle(handle),
            None => Ok(()),
        }
    }

    /// Writes one output report.
    ///
    /// `report` must start with the report ID and is zero padded to
    /// [`output_length`](Self::output_length), because Windows only accepts
    /// buffers of exactly that size. Returns the number of bytes Windows
    /// reported as written.
    pub fn write_report(&self, report: &[u8]) -> Result<usize> {
        let handle = self.handle()?;
        let buffer = report_buffer(report, usize::from(self.output_length), ReportKind::Output)?;

        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                buffer.as_ptr(),
                buffer.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::last_os("WriteFile"));
        }
        Ok(written as usize)
    }

    /// Reads one input report, blocking until the device sends one.
    ///
    /// The returned slice is at most [`input_length`](Self::input_length) bytes
    /// long and starts with the report ID.
    pub fn read_report(&self) -> Result<Vec<u8>> {
        let handle = self.handle()?;
        if self.input_length == 0 {
            return Err(Error::NoReport(ReportKind::Input));
        }

        let mut buffer = vec![0u8; usize::from(self.input_length)];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::last_os("ReadFile"));
        }

        buffer.truncate(read as usize);
        Ok(buffer)
    }

    /// Writes one feature report with `HidD_SetFeature`.
    ///
    /// `report` must start with the report ID and is zero padded to
    /// [`feature_length`](Self::feature_length).
    pub fn write_feature_report(&self, report: &[u8]) -> Result<usize> {
        let handle = self.handle()?;
        let buffer = report_buffer(
            report,
            usize::from(self.feature_length),
            ReportKind::Feature,
        )?;

        let ok = unsafe {
            HidD_SetFeature(
                handle,
                buffer.as_ptr().cast::<c_void>(),
                buffer.len() as u32,
            )
        };
        if !ok {
            return Err(Error::last_os("HidD_SetFeature"));
        }
        Ok(buffer.len())
    }

    /// Reads the feature report identified by `report_id` with
    /// `HidD_GetFeature`.
    ///
    /// The returned slice is [`feature_length`](Self::feature_length) bytes
    /// long and starts with the report ID. Pass 0 for devices that do not use
    /// report IDs.
    pub fn read_feature_report(&self, report_id: u8) -> Result<Vec<u8>> {
        let handle = self.handle()?;
        if self.feature_length == 0 {
            return Err(Error::NoReport(ReportKind::Feature));
        }

        let mut buffer = vec![0u8; usize::from(self.feature_length)];
        buffer[0] = report_id;

        let ok = unsafe {
            HidD_GetFeature(
                handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
            )
        };
        if !ok {
            return Err(Error::last_os("HidD_GetFeature"));
        }
        Ok(buffer)
    }

    fn handle(&self) -> Result<HANDLE> {
        self.handle.ok_or(Error::NotOpen)
    }
}

impl Drop for HidDeviceEndpoint {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Pads `report` with zeros to `length`, the buffer size Windows requires for
/// the given report type.
fn report_buffer(report: &[u8], length: usize, kind: ReportKind) -> Result<Vec<u8>> {
    if length == 0 {
        return Err(Error::NoReport(kind));
    }
    if report.is_empty() {
        return Err(Error::EmptyReport(kind));
    }
    if report.len() > length {
        return Err(Error::ReportTooLong {
            kind,
            len: report.len(),
            max: length,
        });
    }

    let mut buffer = vec![0u8; length];
    buffer[..report.len()].copy_from_slice(report);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Case {
        name: &'static str,
        report: Vec<u8>,
        length: usize,
        want_error: bool,
    }

    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "padded",
                report: vec![0x55, 0x65],
                length: 64,
                want_error: false,
            },
            Case {
                name: "exact",
                report: vec![0x07, 0x01, 0x02],
                length: 3,
                want_error: false,
            },
            Case {
                name: "too long",
                report: vec![0x01, 0x02, 0x03],
                length: 2,
                want_error: true,
            },
            Case {
                name: "empty report",
                report: Vec::new(),
                length: 4,
                want_error: true,
            },
            Case {
                name: "no report id",
                report: vec![0x01],
                length: 0,
                want_error: true,
            },
        ]
    }

    #[test]
    fn report_buffer_pads_and_validates() {
        for case in cases() {
            let got = report_buffer(&case.report, case.length, ReportKind::Output);

            if case.want_error {
                assert!(got.is_err(), "{}: expected an error", case.name);
                continue;
            }

            let got = got.unwrap_or_else(|error| panic!("{}: {error}", case.name));
            assert_eq!(got.len(), case.length, "{}: wrong length", case.name);
            assert_eq!(
                &got[..case.report.len()],
                &case.report[..],
                "{}: report bytes changed",
                case.name
            );
            assert!(
                got[case.report.len()..].iter().all(|&byte| byte == 0),
                "{}: padding is not zeroed",
                case.name
            );
        }
    }

    #[test]
    fn report_buffer_reports_the_kind() {
        let error = report_buffer(&[1, 2, 3], 2, ReportKind::Feature).unwrap_err();
        match error {
            Error::ReportTooLong { kind, len, max } => {
                assert_eq!(kind, ReportKind::Feature);
                assert_eq!(len, 3);
                assert_eq!(max, 2);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn closing_twice_is_a_no_op() {
        let mut endpoint = HidDeviceEndpoint {
            path: String::new(),
            usage: 0,
            usage_page: 0,
            input_length: 0,
            output_length: 0,
            feature_length: 0,
            handle: None,
        };
        assert!(!endpoint.is_open());
        assert!(endpoint.close().is_ok());
        assert!(endpoint.close().is_ok());
    }

    #[test]
    fn report_methods_require_an_open_endpoint() {
        let endpoint = HidDeviceEndpoint {
            path: String::new(),
            usage: 0,
            usage_page: 0,
            input_length: 8,
            output_length: 8,
            feature_length: 8,
            handle: None,
        };

        assert!(matches!(endpoint.write_report(&[0]), Err(Error::NotOpen)));
        assert!(matches!(endpoint.read_report(), Err(Error::NotOpen)));
        assert!(matches!(
            endpoint.write_feature_report(&[0]),
            Err(Error::NotOpen)
        ));
        assert!(matches!(
            endpoint.read_feature_report(0),
            Err(Error::NotOpen)
        ));
    }

    #[test]
    fn report_methods_check_the_report_kind() {
        // The handle check comes first, so use report_buffer to verify the
        // "no report of that kind" path without a device.
        assert!(matches!(
            report_buffer(&[0], 0, ReportKind::Output),
            Err(Error::NoReport(ReportKind::Output))
        ));
        assert!(matches!(
            report_buffer(&[], 8, ReportKind::Feature),
            Err(Error::EmptyReport(ReportKind::Feature))
        ));
    }
}
