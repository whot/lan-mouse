use input_event::{Event as InputEvent, KeyboardEvent, PointerEvent};
use num_enum::{IntoPrimitive, TryFromPrimitive, TryFromPrimitiveError};
use paste::paste;
use std::{
    fmt::{Debug, Display, Formatter},
    mem::size_of,
};
use thiserror::Error;

/// Max length of a mime type string in bytes
pub const MAX_MIME_TYPE_LEN: usize = 64;

/// MAx clipboard data chunk size in bytes
pub const MAX_CLIPBOARD_CHUNK_SIZE: usize = 256;

/// defines the maximum size an encoded event can take up
/// this is currently the clipboard data event
/// type: u8, serial: u32, offset: u32, data_len: u8, data: [u8;256]
pub const MAX_EVENT_SIZE: usize =
    size_of::<u8>() + size_of::<u32>() * 2 + size_of::<u8>() + MAX_CLIPBOARD_CHUNK_SIZE;

/// error type for protocol violations
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// event type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidEventId(#[from] TryFromPrimitiveError<EventType>),
    /// position type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidPosition(#[from] TryFromPrimitiveError<Position>),
}

/// Position of a client
#[derive(Clone, Copy, Debug, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum Position {
    Left,
    Right,
    Top,
    Bottom,
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let pos = match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        write!(f, "{pos}")
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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

/// main lan-mouse protocol event type
#[derive(Clone, Copy, Debug)]
pub enum ProtoEvent {
    /// notify a client that the cursor entered its region at the given position
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Enter(Position),
    /// notify a client that the cursor left its region
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Leave(u32),
    /// acknowledge of an [`ProtoEvent::Enter`] or [`ProtoEvent::Leave`] event
    Ack(u32),
    /// Input event
    Input(InputEvent),
    /// Ping event for tracking unresponsive clients.
    /// A client has to respond with [`ProtoEvent::Pong`].
    Ping,
    /// Response to [`ProtoEvent::Ping`], true if emulation is enabled / available
    Pong(bool),
    /// A clipboard event
    Clipboard(ClipboardEvent),
}

impl Display for ProtoEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoEvent::Enter(s) => write!(f, "Enter({s})"),
            ProtoEvent::Leave(s) => write!(f, "Leave({s})"),
            ProtoEvent::Ack(s) => write!(f, "Ack({s})"),
            ProtoEvent::Input(e) => write!(f, "{e}"),
            ProtoEvent::Ping => write!(f, "ping"),
            ProtoEvent::Pong(alive) => {
                write!(
                    f,
                    "pong: {}",
                    if *alive { "alive" } else { "not available" }
                )
            }
            ProtoEvent::Clipboard(e) => write!(f, "{e}"),
        }
    }
}

#[derive(TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum EventType {
    PointerMotion,
    PointerButton,
    PointerAxis,
    PointerAxisValue120,
    KeyboardKey,
    KeyboardModifiers,
    Ping,
    Pong,
    Enter,
    Leave,
    Ack,
    ClipboardNotify,
    ClipboardNotifyDone,
    ClipboardRequest,
    ClipboardData,
    ClipboardDataDone,
}

impl ProtoEvent {
    fn event_type(&self) -> EventType {
        match self {
            ProtoEvent::Input(e) => match e {
                InputEvent::Pointer(p) => match p {
                    PointerEvent::Motion { .. } => EventType::PointerMotion,
                    PointerEvent::Button { .. } => EventType::PointerButton,
                    PointerEvent::Axis { .. } => EventType::PointerAxis,
                    PointerEvent::AxisDiscrete120 { .. } => EventType::PointerAxisValue120,
                },
                InputEvent::Keyboard(k) => match k {
                    KeyboardEvent::Key { .. } => EventType::KeyboardKey,
                    KeyboardEvent::Modifiers { .. } => EventType::KeyboardModifiers,
                },
            },
            ProtoEvent::Ping => EventType::Ping,
            ProtoEvent::Pong(_) => EventType::Pong,
            ProtoEvent::Enter(_) => EventType::Enter,
            ProtoEvent::Leave(_) => EventType::Leave,
            ProtoEvent::Ack(_) => EventType::Ack,
            ProtoEvent::Clipboard(e) => match e {
                ClipboardEvent::Notify { .. } => EventType::ClipboardNotify,
                ClipboardEvent::NotifyDone { .. } => EventType::ClipboardNotifyDone,
                ClipboardEvent::Request { .. } => EventType::ClipboardRequest,
                ClipboardEvent::Data { .. } => EventType::ClipboardData,
                ClipboardEvent::DataDone { .. } => EventType::ClipboardDataDone,
            },
        }
    }
}

impl TryFrom<[u8; MAX_EVENT_SIZE]> for ProtoEvent {
    type Error = ProtocolError;

    fn try_from(buf: [u8; MAX_EVENT_SIZE]) -> Result<Self, Self::Error> {
        let mut buf = &buf[..];
        let event_type = decode_u8(&mut buf)?;
        match EventType::try_from(event_type)? {
            EventType::PointerMotion => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Motion {
                    time: decode_u32(&mut buf)?,
                    dx: decode_f64(&mut buf)?,
                    dy: decode_f64(&mut buf)?,
                })))
            }
            EventType::PointerButton => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Button {
                    time: decode_u32(&mut buf)?,
                    button: decode_u32(&mut buf)?,
                    state: decode_u32(&mut buf)?,
                })))
            }
            EventType::PointerAxis => Ok(Self::Input(InputEvent::Pointer(PointerEvent::Axis {
                time: decode_u32(&mut buf)?,
                axis: decode_u8(&mut buf)?,
                value: decode_f64(&mut buf)?,
            }))),
            EventType::PointerAxisValue120 => Ok(Self::Input(InputEvent::Pointer(
                PointerEvent::AxisDiscrete120 {
                    axis: decode_u8(&mut buf)?,
                    value: decode_i32(&mut buf)?,
                },
            ))),
            EventType::KeyboardKey => Ok(Self::Input(InputEvent::Keyboard(KeyboardEvent::Key {
                time: decode_u32(&mut buf)?,
                key: decode_u32(&mut buf)?,
                state: decode_u8(&mut buf)?,
            }))),
            EventType::KeyboardModifiers => Ok(Self::Input(InputEvent::Keyboard(
                KeyboardEvent::Modifiers {
                    depressed: decode_u32(&mut buf)?,
                    latched: decode_u32(&mut buf)?,
                    locked: decode_u32(&mut buf)?,
                    group: decode_u32(&mut buf)?,
                },
            ))),
            EventType::Ping => Ok(Self::Ping),
            EventType::Pong => Ok(Self::Pong(decode_u8(&mut buf)? != 0)),
            EventType::Enter => Ok(Self::Enter(decode_u8(&mut buf)?.try_into()?)),
            EventType::Leave => Ok(Self::Leave(decode_u32(&mut buf)?)),
            EventType::Ack => Ok(Self::Ack(decode_u32(&mut buf)?)),
            EventType::ClipboardNotify => {
                let serial = decode_u32(&mut buf)?;
                let mime_type = decode_bytes::<MAX_MIME_TYPE_LEN>(&mut buf)?;
                Ok(Self::Clipboard(ClipboardEvent::Notify {
                    serial,
                    mime_type,
                }))
            }
            EventType::ClipboardNotifyDone => {
                let serial = decode_u32(&mut buf)?;
                Ok(Self::Clipboard(ClipboardEvent::NotifyDone { serial }))
            }
            EventType::ClipboardRequest => {
                let serial = decode_u32(&mut buf)?;
                let mime_type = decode_bytes::<MAX_MIME_TYPE_LEN>(&mut buf)?;
                Ok(Self::Clipboard(ClipboardEvent::Request {
                    serial,
                    mime_type,
                }))
            }
            EventType::ClipboardData => {
                let serial = decode_u32(&mut buf)?;
                let offset = decode_u32(&mut buf)?;
                let data_len = decode_u8(&mut buf)?;
                let data = decode_bytes::<MAX_CLIPBOARD_CHUNK_SIZE>(&mut buf)?;
                Ok(Self::Clipboard(ClipboardEvent::Data {
                    serial,
                    offset,
                    data_len,
                    data,
                }))
            }
            EventType::ClipboardDataDone => {
                let serial = decode_u32(&mut buf)?;
                Ok(Self::Clipboard(ClipboardEvent::DataDone { serial }))
            }
        }
    }
}

impl From<ProtoEvent> for ([u8; MAX_EVENT_SIZE], usize) {
    fn from(event: ProtoEvent) -> Self {
        let mut buf = [0u8; MAX_EVENT_SIZE];
        let mut len = 0usize;
        {
            let mut buf = &mut buf[..];
            let buf = &mut buf;
            let len = &mut len;
            encode_u8(buf, len, event.event_type() as u8);
            match event {
                ProtoEvent::Input(event) => match event {
                    InputEvent::Pointer(p) => match p {
                        PointerEvent::Motion { time, dx, dy } => {
                            encode_u32(buf, len, time);
                            encode_f64(buf, len, dx);
                            encode_f64(buf, len, dy);
                        }
                        PointerEvent::Button {
                            time,
                            button,
                            state,
                        } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, button);
                            encode_u32(buf, len, state);
                        }
                        PointerEvent::Axis { time, axis, value } => {
                            encode_u32(buf, len, time);
                            encode_u8(buf, len, axis);
                            encode_f64(buf, len, value);
                        }
                        PointerEvent::AxisDiscrete120 { axis, value } => {
                            encode_u8(buf, len, axis);
                            encode_i32(buf, len, value);
                        }
                    },
                    InputEvent::Keyboard(k) => match k {
                        KeyboardEvent::Key { time, key, state } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, key);
                            encode_u8(buf, len, state);
                        }
                        KeyboardEvent::Modifiers {
                            depressed,
                            latched,
                            locked,
                            group,
                        } => {
                            encode_u32(buf, len, depressed);
                            encode_u32(buf, len, latched);
                            encode_u32(buf, len, locked);
                            encode_u32(buf, len, group);
                        }
                    },
                },
                ProtoEvent::Ping => {}
                ProtoEvent::Pong(alive) => encode_u8(buf, len, alive as u8),
                ProtoEvent::Enter(pos) => encode_u8(buf, len, pos as u8),
                ProtoEvent::Leave(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Ack(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Clipboard(event) => match event {
                    ClipboardEvent::Notify { serial, mime_type } => {
                        encode_u32(buf, len, serial);
                        encode_bytes(buf, len, &mime_type);
                    }
                    ClipboardEvent::NotifyDone { serial } => {
                        encode_u32(buf, len, serial);
                    }
                    ClipboardEvent::Request { serial, mime_type } => {
                        encode_u32(buf, len, serial);
                        encode_bytes(buf, len, &mime_type);
                    }
                    ClipboardEvent::Data {
                        serial,
                        offset,
                        data_len,
                        data,
                    } => {
                        encode_u32(buf, len, serial);
                        encode_u32(buf, len, offset);
                        encode_u8(buf, len, data_len);
                        encode_bytes(buf, len, &data);
                    }
                    ClipboardEvent::DataDone { serial } => {
                        encode_u32(buf, len, serial);
                    }
                },
            }
        }
        (buf, len)
    }
}

macro_rules! decode_impl {
    ($t:ty) => {
        paste! {
            fn [<decode_ $t>](data: &mut &[u8]) -> Result<$t, ProtocolError> {
                let (int_bytes, rest) = data.split_at(size_of::<$t>());
                *data = rest;
                Ok($t::from_be_bytes(int_bytes.try_into().unwrap()))
            }
        }
    };
}

decode_impl!(u8);
decode_impl!(u32);
decode_impl!(i32);
decode_impl!(f64);

fn decode_bytes<const N: usize>(data: &mut &[u8]) -> Result<[u8; N], ProtocolError> {
    let (bytes, rest) = data.split_at(N);
    *data = rest;
    let mut result = [0u8; N];
    result.copy_from_slice(bytes);
    Ok(result)
}

macro_rules! encode_impl {
    ($t:ty) => {
        paste! {
            fn [<encode_ $t>](buf: &mut &mut [u8], amt: &mut usize, n: $t) {
                let src = n.to_be_bytes();
                let data = std::mem::take(buf);
                let (int_bytes, rest) = data.split_at_mut(size_of::<$t>());
                int_bytes.copy_from_slice(&src);
                *amt += size_of::<$t>();
                *buf = rest
            }
        }
    };
}

encode_impl!(u8);
encode_impl!(u32);
encode_impl!(i32);
encode_impl!(f64);

fn encode_bytes<const N: usize>(buf: &mut &mut [u8], amt: &mut usize, bytes: &[u8; N]) {
    let data = std::mem::take(buf);
    let (dest, rest) = data.split_at_mut(N);
    dest.copy_from_slice(bytes);
    *amt += N;
    *buf = rest;
}
