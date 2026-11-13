//! Error type shared by the whole crate.

use std::error;
use std::fmt;
use std::io;

/// The kind of a HID report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportKind {
    /// An input report, read from the device.
    Input,
    /// An output report, written to the device.
    Output,
    /// A feature report, exchanged in both directions.
    Feature,
}

impl fmt::Display for ReportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Feature => "feature",
        })
    }
}

/// Errors returned by `rshid_win`.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The endpoint has not been opened, or it was closed again.
    NotOpen,

    /// The endpoint has no report of the requested kind.
    NoReport(ReportKind),

    /// The report passed to a write function is empty.
    EmptyReport(ReportKind),

    /// The report is longer than the endpoint's report buffer.
    ReportTooLong {
        /// The kind of report that was passed.
        kind: ReportKind,
        /// Length of the report that was passed.
        len: usize,
        /// Report length the endpoint accepts.
        max: usize,
    },

    /// A Win32 call failed.
    Win32 {
        /// Name of the Win32 function that failed.
        call: &'static str,
        /// The operating system error it reported.
        source: io::Error,
    },

    /// Windows returned data that could not be interpreted.
    InvalidData(&'static str),
}

impl Error {
    /// Builds a [`Error::Win32`] from the calling thread's last-error code.
    pub(crate) fn last_os(call: &'static str) -> Self {
        Self::Win32 {
            call,
            source: io::Error::last_os_error(),
        }
    }

    /// Builds a [`Error::Win32`] from an explicit error code.
    pub(crate) fn win32(call: &'static str, source: io::Error) -> Self {
        Self::Win32 { call, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen => f.write_str("rshid_win: endpoint is not open"),
            Self::NoReport(kind) => write!(f, "rshid_win: endpoint has no {kind} report"),
            Self::EmptyReport(kind) => write!(f, "rshid_win: {kind} report is empty"),
            Self::ReportTooLong { kind, len, max } => write!(
                f,
                "rshid_win: {kind} report is {len} bytes, endpoint accepts {max}"
            ),
            Self::Win32 { call, source } => write!(f, "rshid_win: {call}: {source}"),
            Self::InvalidData(message) => f.write_str(message),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Win32 { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result type used by all fallible functions of this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn report_kind_display() {
        assert_eq!(ReportKind::Input.to_string(), "input");
        assert_eq!(ReportKind::Output.to_string(), "output");
        assert_eq!(ReportKind::Feature.to_string(), "feature");
    }

    #[test]
    fn windows_error_exposes_source() {
        let error = Error::win32("WriteFile", io::Error::from_raw_os_error(1));
        assert!(error.source().is_some());
        assert!(error.to_string().contains("WriteFile"));
    }
}
