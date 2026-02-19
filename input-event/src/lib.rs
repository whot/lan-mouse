use std::fmt::{self, Display};

pub mod error;
pub mod scancode;

#[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
mod libei;

/// Max length of a mime type string in bytes
pub const MAX_MIME_TYPE_LEN: usize = 64;

/// MAx clipboard data chunk size in bytes
pub const MAX_CLIPBOARD_CHUNK_SIZE: usize = 256;

// FIXME
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
pub const BTN_BACK: u32 = 0x113;
pub const BTN_FORWARD: u32 = 0x114;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PointerEvent {
    /// relative motion event
    Motion { time: u32, dx: f64, dy: f64 },
    /// mouse button event
    Button { time: u32, button: u32, state: u32 },
    /// axis event, scroll event for touchpads
    Axis { time: u32, axis: u8, value: f64 },
    /// discrete axis event, scroll event for mice - 120 = one scroll tick
    AxisDiscrete120 { axis: u8, value: i32 },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum KeyboardEvent {
    /// a key press / release event
    Key { time: u32, key: u32, state: u8 },
    /// modifiers changed state
    Modifiers {
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    },
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Event {
    /// pointer event (motion / button / axis)
    Pointer(PointerEvent),
    /// keyboard events (key / modifiers)
    Keyboard(KeyboardEvent),
}

impl Display for PointerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PointerEvent::Motion { time: _, dx, dy } => write!(f, "motion({dx},{dy})"),
            PointerEvent::Button {
                time: _,
                button,
                state,
            } => {
                let str = match *button {
                    BTN_LEFT => Some("left"),
                    BTN_RIGHT => Some("right"),
                    BTN_MIDDLE => Some("middle"),
                    BTN_FORWARD => Some("forward"),
                    BTN_BACK => Some("back"),
                    _ => None,
                };
                if let Some(button) = str {
                    write!(f, "button({button}, {state})")
                } else {
                    write!(f, "button({button}, {state}")
                }
            }
            PointerEvent::Axis {
                time: _,
                axis,
                value,
            } => write!(f, "scroll({axis}, {value})"),
            PointerEvent::AxisDiscrete120 { axis, value } => {
                write!(f, "scroll-120 ({axis}, {value})")
            }
        }
    }
}

impl Display for KeyboardEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyboardEvent::Key {
                time: _,
                key,
                state,
            } => {
                let scan = scancode::Linux::try_from(*key);
                if let Ok(scan) = scan {
                    write!(f, "key({scan:?}, {state})")
                } else {
                    write!(f, "key({key}, {state})")
                }
            }
            KeyboardEvent::Modifiers {
                depressed: mods_depressed,
                latched: mods_latched,
                locked: mods_locked,
                group,
            } => write!(
                f,
                "modifiers({mods_depressed},{mods_latched},{mods_locked},{group})"
            ),
        }
    }
}

impl Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Pointer(p) => write!(f, "{p}"),
            Event::Keyboard(k) => write!(f, "{k}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ClipboardEvent {
    /// Notify a client that clipboard content is available with the given mime type.
    ///
    /// This event is sent by the owner of the clipboard context.
    Notify {
        /// Serial number for this clipboard notification session
        serial: u32,
        /// Mime type string (null-terminated)
        mime_type: [u8; MAX_MIME_TYPE_LEN],
    },
    /// Signals the end of clipboard mime type notifications
    ///
    /// The serial number must match the previous ClipboardNotify (if any).
    /// An empty sequence consisting of just a ClipboardNotifyDone indicates
    /// that the clipboard content was lost.
    NotifyDone {
        /// Serial number of the completed notification
        serial: u32,
    },
    /// Request clipboard data for a specific mime type.
    ///
    /// This is sent *to* the owner of the clipboard context
    Request {
        /// Serial number for this request
        serial: u32,
        /// Mime type string being requested (null-terminated)
        mime_type: [u8; MAX_MIME_TYPE_LEN],
    },
    /// Clipboard data chunk - multiple events sent for large data
    /// in response to a ClipboardRequest
    ///
    /// The serial must match the ClipboardRequest serial.
    ///
    /// This event is sent by the owner of the clipboard context.
    Data {
        /// Serial number identifying this clipboard transfer
        serial: u32,
        /// Byte offset of this chunk in the complete data
        offset: u32,
        /// Number of valid bytes in the data field (1-256)
        data_len: u8,
        /// Clipboard data bytes for this chunk
        data: [u8; MAX_CLIPBOARD_CHUNK_SIZE],
    },
    /// Signals the end of clipboard data transfer
    ///
    /// The serial must match the ClipboardRequest serial.
    /// An empty sequence consisting of only ClipboardDataDone
    /// indicates that no data for this mime type was available.
    DataDone {
        /// Serial number of the completed clipboard transfer
        serial: u32,
    },
}

impl Display for ClipboardEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardEvent::Notify { serial, mime_type } => {
                let mime_str = std::str::from_utf8(mime_type)
                    .unwrap_or("<invalid>")
                    .trim_end_matches('\0');
                write!(f, "ClipboardNotify(serial={}, mime={})", serial, mime_str)
            }
            ClipboardEvent::NotifyDone { serial } => {
                write!(f, "ClipboardNotifyDone(serial={})", serial)
            }
            ClipboardEvent::Request { serial, mime_type } => {
                let mime_str = std::str::from_utf8(mime_type)
                    .unwrap_or("<invalid>")
                    .trim_end_matches('\0');
                write!(f, "ClipboardRequest(serial={}, mime={})", serial, mime_str)
            }
            ClipboardEvent::Data {
                serial,
                offset,
                data_len,
                ..
            } => {
                write!(
                    f,
                    "ClipboardData(serial={}, offset={}, len={})",
                    serial, offset, data_len
                )
            }
            ClipboardEvent::DataDone { serial } => {
                write!(f, "ClipboardDataDone(serial={})", serial)
            }
        }
    }
}
