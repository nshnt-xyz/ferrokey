//! Sends events over a libei connection from the portal's `ConnectToEIS`.
//!
//! libei streams events over a socket instead of making a D-Bus round trip
//! for each one. The compositor offers devices with capabilities (pointer,
//! keyboard, touchscreen, ...); each event goes to a device that has the
//! capability it needs, followed by a frame that commits it.

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use ashpd::enumflags2::BitFlags;
use ferrokey_protocol::InputEvent;
use ferrokey_protocol::keys::{self, KEY_LEFTSHIFT};
use futures_util::StreamExt;
use reis::ei;
use reis::event::{Connection, Device, DeviceCapability, EiEvent};
use reis::tokio::EiConvertEventStream;
use tokio::sync::mpsc;

use super::dbus::button_code;
use super::layout::{Area, Layout};
use super::{BoxError, InjectError, NO_SCREEN, Reporter};

/// How long [`Eis::connect`] waits for the compositor to offer devices.
const DEVICE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct Eis {
    connection: Connection,
    events: EiConvertEventStream,
    devices: Vec<Device>,
    /// Devices that may send events now.
    resumed: HashSet<Device>,
    /// Numbers each `start_emulating`; it must go up every time.
    sequence: u32,
}

impl Eis {
    /// Performs the libei handshake on `stream` and waits briefly for the
    /// compositor to offer a pointer and a keyboard.
    pub async fn connect(stream: UnixStream) -> Result<Self, BoxError> {
        let context = ei::Context::new(stream)?;
        let (connection, events) = context
            .handshake_tokio("ferrokey", ei::handshake::ContextType::Sender)
            .await?;
        let mut eis = Self {
            connection,
            events,
            devices: Vec::new(),
            resumed: HashSet::new(),
            sequence: 0,
        };
        let wait = async {
            while !(eis.can(DeviceCapability::Pointer) && eis.can(DeviceCapability::Keyboard)) {
                let Some(event) = eis.events.next().await else {
                    return Err("libei connection closed during setup".into());
                };
                eis.handle(event)?;
            }
            Ok::<_, BoxError>(())
        };
        // Events for devices that haven't shown up yet are skipped with a
        // message, so carry on if some never do.
        if let Ok(result) = tokio::time::timeout(DEVICE_TIMEOUT, wait).await {
            result?;
        }
        Ok(eis)
    }

    /// Forwards queued events until every sender is dropped or the
    /// compositor closes the connection.
    pub async fn run(mut self, mut events: mpsc::UnboundedReceiver<InputEvent>) {
        let mut reporter = Reporter::default();
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { break };
                    let result = self.inject(event);
                    reporter.report(event, result);
                }
                ei_event = self.events.next() => {
                    let result = match ei_event {
                        Some(ei_event) => self.handle(ei_event),
                        None => Err("libei connection closed".into()),
                    };
                    if let Err(err) = result {
                        eprintln!("ferrokey-input: {err}");
                        break;
                    }
                }
            }
        }
    }

    /// Tracks the seats and devices the compositor offers.
    fn handle(&mut self, event: Result<EiEvent, reis::Error>) -> Result<(), BoxError> {
        match event? {
            EiEvent::SeatAdded(added) => {
                // Capabilities the seat doesn't offer are left out.
                added.seat.bind_capabilities(BitFlags::all());
            }
            EiEvent::DeviceAdded(added) => self.devices.push(added.device),
            EiEvent::DeviceResumed(resumed) => {
                resumed
                    .device
                    .device()
                    .start_emulating(resumed.serial, self.sequence);
                self.sequence = self.sequence.wrapping_add(1);
                self.resumed.insert(resumed.device);
            }
            EiEvent::DevicePaused(paused) => {
                self.resumed.remove(&paused.device);
            }
            EiEvent::DeviceRemoved(removed) => {
                self.resumed.remove(&removed.device);
                self.devices.retain(|device| *device != removed.device);
            }
            EiEvent::Disconnected(disconnected) => {
                return Err(format!(
                    "compositor closed the libei connection ({:?}{})",
                    disconnected.reason,
                    disconnected
                        .explanation
                        .map(|why| format!(": {why}"))
                        .unwrap_or_default()
                )
                .into());
            }
            _ => {}
        }
        self.connection.flush()?;
        Ok(())
    }

    fn device(&self, capability: DeviceCapability) -> Option<&Device> {
        self.devices
            .iter()
            .find(|device| self.resumed.contains(*device) && device.has_capability(capability))
    }

    fn can(&self, capability: DeviceCapability) -> bool {
        self.device(capability).is_some()
    }

    /// A device with `capability` and its interface for it.
    fn interface<T: ei::Interface>(
        &self,
        capability: DeviceCapability,
    ) -> Result<(Device, T), InjectError> {
        let missing = InjectError::Unavailable(missing(capability));
        let device = self.device(capability).ok_or(missing)?;
        let interface = device.interface().ok_or(InjectError::Unavailable(
            "a libei device is missing an interface it advertised",
        ))?;
        Ok((device.clone(), interface))
    }

    /// Commits the events sent to `device` since the last frame.
    fn frame(&self, device: &Device) -> Result<(), InjectError> {
        device
            .device()
            .frame(self.connection.serial(), now_micros());
        self.connection.flush()?;
        Ok(())
    }

    fn inject(&self, event: InputEvent) -> Result<(), InjectError> {
        match event {
            InputEvent::MouseMove { dx, dy } => {
                let (device, pointer) = self.interface::<ei::Pointer>(DeviceCapability::Pointer)?;
                pointer.motion_relative(dx as f32, dy as f32);
                self.frame(&device)
            }
            InputEvent::MouseButton { button, pressed } => {
                let (device, buttons) = self.interface::<ei::Button>(DeviceCapability::Button)?;
                let state = if pressed {
                    ei::button::ButtonState::Press
                } else {
                    ei::button::ButtonState::Released
                };
                buttons.button(button_code(button), state);
                self.frame(&device)
            }
            InputEvent::Scroll { dx, dy } => {
                let (device, scroll) = self.interface::<ei::Scroll>(DeviceCapability::Scroll)?;
                // libei counts a wheel detent as 120.
                scroll.scroll_discrete(dx.saturating_mul(120), dy.saturating_mul(120));
                self.frame(&device)
            }
            InputEvent::SmoothScroll { dx, dy, finish } => {
                let (device, scroll) = self.interface::<ei::Scroll>(DeviceCapability::Scroll)?;
                if dx != 0.0 || dy != 0.0 {
                    scroll.scroll(dx, dy);
                    self.frame(&device)?;
                }
                if finish {
                    // A stop may not share a frame with motion on its axis.
                    scroll.scroll_stop(1, 1, 0);
                    self.frame(&device)?;
                }
                Ok(())
            }
            InputEvent::Key { code, pressed } => self.key(code, pressed),
            InputEvent::Keysym { keysym, pressed } => {
                if let Ok((device, text)) = self.interface::<ei::Text>(DeviceCapability::Text) {
                    text.keysym(keysym, key_state(pressed));
                    return self.frame(&device);
                }
                // Without text input, fall back to the US QWERTY key for it.
                let (code, shift) = keys::keysym_char(keysym).and_then(keys::char_key).ok_or(
                    InjectError::Unavailable(
                        "the compositor's libei has no text input, so only characters \
                         on a US keyboard can be typed",
                    ),
                )?;
                if shift && pressed {
                    self.key(KEY_LEFTSHIFT, true)?;
                }
                self.key(code, pressed)?;
                if shift && !pressed {
                    self.key(KEY_LEFTSHIFT, false)?;
                }
                Ok(())
            }
            InputEvent::MouseMoveAbsolute { x, y } => {
                let (device, pointer) =
                    self.interface::<ei::PointerAbsolute>(DeviceCapability::PointerAbsolute)?;
                let (x, y) = locate(&device, x, y)?;
                pointer.motion_absolute(x, y);
                self.frame(&device)
            }
            InputEvent::TouchDown { id, x, y } => {
                let (device, touch) = self.interface::<ei::Touchscreen>(DeviceCapability::Touch)?;
                let (x, y) = locate(&device, x, y)?;
                touch.down(id, x, y);
                self.frame(&device)
            }
            InputEvent::TouchMove { id, x, y } => {
                let (device, touch) = self.interface::<ei::Touchscreen>(DeviceCapability::Touch)?;
                let (x, y) = locate(&device, x, y)?;
                touch.motion(id, x, y);
                self.frame(&device)
            }
            InputEvent::TouchUp { id } => {
                let (device, touch) = self.interface::<ei::Touchscreen>(DeviceCapability::Touch)?;
                touch.up(id);
                self.frame(&device)
            }
        }
    }

    fn key(&self, code: u32, pressed: bool) -> Result<(), InjectError> {
        let (device, keyboard) = self.interface::<ei::Keyboard>(DeviceCapability::Keyboard)?;
        keyboard.key(code, key_state(pressed));
        self.frame(&device)
    }
}

/// Places an absolute position, in [`InputEvent::ABSOLUTE_MAX`] units, on
/// `device`'s regions, in global logical pixels as libei wants them.
fn locate(device: &Device, x: u16, y: u16) -> Result<(f32, f32), InjectError> {
    let max = f32::from(InputEvent::ABSOLUTE_MAX);
    let (x, y) = (f32::from(x) / max, f32::from(y) / max);
    let layout = Layout::new(device.regions().iter().map(|region| Area {
        x: region.x.into(),
        y: region.y.into(),
        width: region.width.into(),
        height: region.height.into(),
    }));
    let (x, y) = layout
        .locate(x, y)
        .ok_or(InjectError::Unavailable(NO_SCREEN))?;
    Ok((x as f32, y as f32))
}

fn missing(capability: DeviceCapability) -> &'static str {
    match capability {
        DeviceCapability::Pointer => "the compositor offers no libei pointer",
        DeviceCapability::PointerAbsolute => {
            "the compositor offers no absolute libei pointer; is screen sharing on?"
        }
        DeviceCapability::Keyboard => "the compositor offers no libei keyboard",
        DeviceCapability::Touch => {
            "the compositor offers no libei touchscreen; is screen sharing on?"
        }
        DeviceCapability::Scroll => "the compositor offers no libei scrolling",
        DeviceCapability::Button => "the compositor offers no libei buttons",
        DeviceCapability::Text => "the compositor offers no libei text input",
    }
}

fn key_state(pressed: bool) -> ei::keyboard::KeyState {
    if pressed {
        ei::keyboard::KeyState::Press
    } else {
        ei::keyboard::KeyState::Released
    }
}

/// Frame timestamps are microseconds of `CLOCK_MONOTONIC`.
fn now_micros() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let secs = u64::try_from(now.tv_sec).unwrap_or(0);
    let micros = u64::try_from(now.tv_nsec).unwrap_or(0) / 1000;
    secs * 1_000_000 + micros
}
