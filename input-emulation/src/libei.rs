use futures::{FutureExt, Stream, StreamExt, future};
use std::{
    env, fs, io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};

use ashpd::desktop::{
    PersistMode, Session,
    clipboard::Clipboard,
    remote_desktop::{DeviceType, RemoteDesktop},
};
use async_trait::async_trait;

use reis::{
    ei::{
        self, Button, Keyboard, Pointer, Scroll, button::ButtonState, handshake::ContextType,
        keyboard::KeyState,
    },
    event::{self, Connection, DeviceCapability, DeviceEvent, EiEvent, SeatEvent},
    tokio::EiConvertEventStream,
};

use input_event::{Event, KeyboardEvent, PointerEvent};

use crate::error::EmulationError;

use super::{Emulation, EmulationHandle, error::LibeiEmulationCreationError};

#[derive(Clone, Debug, PartialEq)]
pub enum ClipboardEvent {
    Noop, // Unused, for the compiler so we can terminate the loop in the task
    OwnerChanged((bool, Vec<String>)),
}

#[derive(Clone, Default)]
struct Devices {
    pointer: Arc<RwLock<Option<(ei::Device, ei::Pointer)>>>,
    scroll: Arc<RwLock<Option<(ei::Device, ei::Scroll)>>>,
    button: Arc<RwLock<Option<(ei::Device, ei::Button)>>>,
    keyboard: Arc<RwLock<Option<(ei::Device, ei::Keyboard)>>>,
}

pub(crate) struct LibeiEmulation {
    context: ei::Context,
    conn: event::Connection,
    devices: Devices,
    ei_task: JoinHandle<()>,
    error: Arc<Mutex<Option<EmulationError>>>,
    libei_error: Arc<AtomicBool>,
    _remote_desktop: RemoteDesktop,
    session: Arc<Session<RemoteDesktop>>,
    _clipboard: Option<Arc<Clipboard>>,
    clipboard_task: Option<JoinHandle<Result<ClipboardEvent, EmulationError>>>,
    clipboard_event_rx: Receiver<ClipboardEvent>,
}

/// Get the path to the RemoteDesktop token file
fn get_token_file_path() -> PathBuf {
    let cache_dir = env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = env::var("HOME").expect("HOME not set");
            PathBuf::from(home).join(".cache")
        });

    cache_dir.join("lan-mouse").join("remote-desktop.token")
}

/// Read the RemoteDesktop token from file
fn read_token() -> Option<String> {
    let token_path = get_token_file_path();
    match fs::read_to_string(&token_path) {
        Ok(token) => Some(token.trim().to_string()),
        Err(_) => None,
    }
}

/// Write the RemoteDesktop token to file
fn write_token(token: &str) -> io::Result<()> {
    let token_path = get_token_file_path();
    if let Some(parent) = token_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&token_path, token)?;
    Ok(())
}

async fn get_ei_fd<'a>() -> Result<
    (
        RemoteDesktop,
        Session<RemoteDesktop>,
        Option<Clipboard>,
        OwnedFd,
    ),
    ashpd::Error,
> {
    let remote_desktop = RemoteDesktop::new().await?;

    let restore_token = read_token();

    log::debug!("creating session ...");
    let session = remote_desktop.create_session().await?;

    log::debug!("selecting devices ...");
    remote_desktop
        .select_devices(
            &session,
            DeviceType::Keyboard | DeviceType::Pointer,
            restore_token.as_deref(),
            PersistMode::ExplicitlyRevoked,
        )
        .await?;

    let clipboard = if remote_desktop.version() >= 2 {
        let clipboard = Clipboard::new().await?;
        if let Err(e) = clipboard.request(&session).await {
            log::warn!("failed to request clipboard access: {}", e);
            None
        } else {
            Some(clipboard)
        }
    } else {
        None
    };

    log::info!("requesting permission for input emulation");
    let start_response = remote_desktop.start(&session, None).await?.response()?;

    let clipboard = if start_response.clipboard_enabled() {
        clipboard
    } else {
        log::warn!("RemoteDesktop session: clipboard access disabled");
        None
    };

    // The restore token is only valid once, we need to re-save it each time
    if let Some(token_str) = start_response.restore_token() {
        if let Err(e) = write_token(token_str) {
            log::warn!("failed to save RemoteDesktop token: {}", e);
        }
    }

    let fd = remote_desktop.connect_to_eis(&session).await?;
    Ok((remote_desktop, session, clipboard, fd))
}

/// Called when the Clipboard portal tells us we have a new
/// selection owner and a set of mime types.
///
/// Note: this will also be called when *we* are the selection owner.
async fn handle_clipboard_owner_changed(
    _clipboard: &Clipboard,
    _session: &Session<RemoteDesktop>,
    mime_types: &[String],
    session_is_owner: Option<bool>,
    clipboard_event_tx: &Sender<ClipboardEvent>,
) {
    // TODO: this needs to be implemented
    let is_owner = session_is_owner.unwrap_or(false);
    log::debug!(
        "RemoteDesktop clipboard owner changed - is_owner: {}, mime_types: {:?}",
        is_owner,
        mime_types
    );

    clipboard_event_tx
        .send(ClipboardEvent::OwnerChanged((is_owner, mime_types.into())))
        .await
        .expect("no channel");
}

/// Called when the Clipboard portal wants the clipboard data
/// for a given mime type. Only called if we have previously
/// become the selection owner.
async fn handle_clipboard_transfer(
    _clipboard: &Clipboard,
    _session: &Session<RemoteDesktop>,
    mime_type: String,
    serial: u32,
) {
    // TODO: this needs to be implemented
    log::debug!(
        "RemoteDesktop clipboard transfer - mime_type: {}, serial: {}",
        mime_type,
        serial
    );
}

async fn clipboard_event_task(
    clipboard: Arc<Clipboard>,
    session: Arc<Session<RemoteDesktop>>,
    clipboard_event_tx: Sender<ClipboardEvent>,
) -> Result<ClipboardEvent, EmulationError> {
    log::debug!("starting clipboard event task");

    let clipboard_owner_changed = match clipboard
        .receive_selection_owner_changed::<RemoteDesktop>()
        .await
    {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("failed to receive selection owner changed events: {}", e);
            return Err(e.into());
        }
    };

    let clipboard_transfer = match clipboard
        .receive_selection_transfer::<RemoteDesktop>()
        .await
    {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("failed to receive selection transfer events: {}", e);
            return Err(e.into());
        }
    };

    tokio::pin!(clipboard_owner_changed);
    tokio::pin!(clipboard_transfer);

    loop {
        tokio::select! {
            owner_changed = clipboard_owner_changed.next() => {
                if let Some((_event_session, changed)) = owner_changed {
                    // Note: We ignore the _event_session and use our main session because
                    // they're supposed to be the same anyway
                    handle_clipboard_owner_changed(
                        &clipboard,
                        &session,
                        changed.mime_types(),
                        changed.session_is_owner(),
                        &clipboard_event_tx,
                    ).await;
                } else {
                    log::debug!("clipboard owner changed stream ended");
                    break;
                }
            },
            transfer = clipboard_transfer.next() => {
                if let Some((transfer_session, mime_type, serial)) = transfer {
                    // Use the transfer session for writing
                    handle_clipboard_transfer(
                        &clipboard,
                        &transfer_session,
                        mime_type,
                        serial,
                    ).await;
                } else {
                    log::debug!("clipboard transfer stream ended");
                    break;
                }
            },
        }
    }

    log::debug!("clipboard event task exited");

    Ok(ClipboardEvent::Noop)
}

impl LibeiEmulation {
    pub(crate) async fn new() -> Result<Self, LibeiEmulationCreationError> {
        let (_remote_desktop, session, clipboard, eifd) = get_ei_fd().await?;
        let stream = UnixStream::from(eifd);
        stream.set_nonblocking(true)?;
        let context = ei::Context::new(stream)?;
        let (conn, events) = context
            .handshake_tokio("de.feschber.LanMouse", ContextType::Sender)
            .await?;
        let devices = Devices::default();
        let libei_error = Arc::new(AtomicBool::default());
        let error = Arc::new(Mutex::new(None));
        let ei_handler = ei_task(
            events,
            conn.clone(),
            context.clone(),
            devices.clone(),
            libei_error.clone(),
            error.clone(),
        );
        let ei_task = tokio::task::spawn_local(ei_handler);

        let session = Arc::new(session);

        let (clipboard_event_tx, clipboard_event_rx) = mpsc::channel(1);
        let (clipboard_task, clipboard) = if let Some(clipboard_instance) = clipboard {
            let clipboard = Arc::new(clipboard_instance);
            let task = tokio::task::spawn_local(clipboard_event_task(
                clipboard.clone(),
                session.clone(),
                clipboard_event_tx,
            ));
            (Some(task), Some(clipboard))
        } else {
            (None, None)
        };

        Ok(Self {
            context,
            conn,
            devices,
            ei_task,
            error,
            libei_error,
            _remote_desktop,
            session,
            _clipboard: clipboard,
            clipboard_task,
            clipboard_event_rx,
        })
    }
}

impl Drop for LibeiEmulation {
    fn drop(&mut self) {
        self.ei_task.abort();
        if let Some(task) = &self.clipboard_task {
            task.abort();
        }
    }
}

impl Stream for LibeiEmulation {
    type Item = Result<ClipboardEvent, EmulationError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(clipboard_task) = self.clipboard_task.as_mut() {
            match clipboard_task.poll_unpin(cx) {
                Poll::Ready(r) => match r.expect("failed to join") {
                    Ok(event) => Poll::Ready(Some(Ok(event))),
                    Err(e) => Poll::Ready(Some(Err(e))),
                },
                Poll::Pending => self
                    .clipboard_event_rx
                    .poll_recv(cx)
                    .map(|e| e.map(Result::Ok)),
            }
        } else {
            Poll::Ready(None)
        }
    }
}

#[async_trait]
impl Emulation for LibeiEmulation {
    async fn consume(
        &mut self,
        event: Event,
        _handle: EmulationHandle,
    ) -> Result<(), EmulationError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        if self.libei_error.load(Ordering::SeqCst) {
            // don't break sending additional events but signal error
            if let Some(e) = self.error.lock().unwrap().take() {
                return Err(e);
            }
        }
        match event {
            Event::Pointer(p) => match p {
                PointerEvent::Motion { time: _, dx, dy } => {
                    let pointer_device = self.devices.pointer.read().unwrap();
                    if let Some((d, p)) = pointer_device.as_ref() {
                        p.motion_relative(dx as f32, dy as f32);
                        d.frame(self.conn.serial(), now);
                    }
                }
                PointerEvent::Button {
                    time: _,
                    button,
                    state,
                } => {
                    let button_device = self.devices.button.read().unwrap();
                    if let Some((d, b)) = button_device.as_ref() {
                        b.button(
                            button,
                            match state {
                                0 => ButtonState::Released,
                                _ => ButtonState::Press,
                            },
                        );
                        d.frame(self.conn.serial(), now);
                    }
                }
                PointerEvent::Axis {
                    time: _,
                    axis,
                    value,
                } => {
                    let scroll_device = self.devices.scroll.read().unwrap();
                    if let Some((d, s)) = scroll_device.as_ref() {
                        match axis {
                            0 => s.scroll(0., value as f32),
                            _ => s.scroll(value as f32, 0.),
                        }
                        d.frame(self.conn.serial(), now);
                    }
                }
                PointerEvent::AxisDiscrete120 { axis, value } => {
                    let scroll_device = self.devices.scroll.read().unwrap();
                    if let Some((d, s)) = scroll_device.as_ref() {
                        match axis {
                            0 => s.scroll_discrete(0, value),
                            _ => s.scroll_discrete(value, 0),
                        }
                        d.frame(self.conn.serial(), now);
                    }
                }
            },
            Event::Keyboard(k) => match k {
                KeyboardEvent::Key {
                    time: _,
                    key,
                    state,
                } => {
                    let keyboard_device = self.devices.keyboard.read().unwrap();
                    if let Some((d, k)) = keyboard_device.as_ref() {
                        k.key(
                            key,
                            match state {
                                0 => KeyState::Released,
                                _ => KeyState::Press,
                            },
                        );
                        d.frame(self.conn.serial(), now);
                    }
                }
                KeyboardEvent::Modifiers { .. } => {}
            },
        }
        self.context
            .flush()
            .map_err(|e| io::Error::new(e.kind(), e))?;
        Ok(())
    }

    async fn create(&mut self, _: EmulationHandle) {}
    async fn destroy(&mut self, _: EmulationHandle) {}

    async fn terminate(&mut self) {
        let _ = self.session.close().await;
        self.ei_task.abort();
    }
}

async fn ei_task(
    mut events: EiConvertEventStream,
    _conn: Connection,
    context: ei::Context,
    devices: Devices,
    libei_error: Arc<AtomicBool>,
    error: Arc<Mutex<Option<EmulationError>>>,
) {
    loop {
        match ei_event_handler(&mut events, &context, &devices).await {
            Ok(()) => {}
            Err(e) => {
                libei_error.store(true, Ordering::SeqCst);
                error.lock().unwrap().replace(e);
                // wait for termination -> otherwise we will loop forever
                future::pending::<()>().await;
            }
        }
    }
}

async fn ei_event_handler(
    events: &mut EiConvertEventStream,
    context: &ei::Context,
    devices: &Devices,
) -> Result<(), EmulationError> {
    loop {
        let event = events.next().await.ok_or(EmulationError::EndOfStream)??;
        const CAPABILITIES: &[DeviceCapability] = &[
            DeviceCapability::Pointer,
            DeviceCapability::PointerAbsolute,
            DeviceCapability::Keyboard,
            DeviceCapability::Touch,
            DeviceCapability::Scroll,
            DeviceCapability::Button,
        ];
        log::debug!("{event:?}");
        match event {
            EiEvent::Disconnected(e) => {
                log::debug!("ei disconnected: {e:?}");
                return Err(EmulationError::EndOfStream);
            }
            EiEvent::SeatAdded(e) => {
                e.seat().bind_capabilities(CAPABILITIES);
            }
            EiEvent::SeatRemoved(e) => {
                log::debug!("seat removed: {:?}", e.seat());
            }
            EiEvent::DeviceAdded(e) => {
                let device_type = e.device().device_type();
                log::debug!("device added: {device_type:?}");
                e.device().device();
                let device = e.device();
                if let Some(pointer) = e.device().interface::<Pointer>() {
                    devices
                        .pointer
                        .write()
                        .unwrap()
                        .replace((device.device().clone(), pointer));
                }
                if let Some(keyboard) = e.device().interface::<Keyboard>() {
                    devices
                        .keyboard
                        .write()
                        .unwrap()
                        .replace((device.device().clone(), keyboard));
                }
                if let Some(scroll) = e.device().interface::<Scroll>() {
                    devices
                        .scroll
                        .write()
                        .unwrap()
                        .replace((device.device().clone(), scroll));
                }
                if let Some(button) = e.device().interface::<Button>() {
                    devices
                        .button
                        .write()
                        .unwrap()
                        .replace((device.device().clone(), button));
                }
            }
            EiEvent::DeviceRemoved(e) => {
                log::debug!("device removed: {:?}", e.device().device_type());
            }
            EiEvent::DevicePaused(e) => {
                log::debug!("device paused: {:?}", e.device().device_type());
            }
            EiEvent::DeviceResumed(e) => {
                log::debug!("device resumed: {:?}", e.device().device_type());
                e.device().device().start_emulating(0, 0);
            }
            EiEvent::KeyboardModifiers(e) => {
                log::debug!("modifiers: {e:?}");
            }
            // only for receiver context
            // EiEvent::Frame(_) => { },
            // EiEvent::DeviceStartEmulating(_) => { },
            // EiEvent::DeviceStopEmulating(_) => { },
            // EiEvent::PointerMotion(_) => { },
            // EiEvent::PointerMotionAbsolute(_) => { },
            // EiEvent::Button(_) => { },
            // EiEvent::ScrollDelta(_) => { },
            // EiEvent::ScrollStop(_) => { },
            // EiEvent::ScrollCancel(_) => { },
            // EiEvent::ScrollDiscrete(_) => { },
            // EiEvent::KeyboardKey(_) => { },
            // EiEvent::TouchDown(_) => { },
            // EiEvent::TouchUp(_) => { },
            // EiEvent::TouchMotion(_) => { },
            _ => unreachable!("unexpected ei event"),
        }
        context.flush().map_err(|e| io::Error::new(e.kind(), e))?;
    }
}
