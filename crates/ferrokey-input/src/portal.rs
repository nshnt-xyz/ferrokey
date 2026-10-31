//! Linux backend built on the xdg-desktop-portal RemoteDesktop interface.
//!
//! Works on any compositor whose portal implements RemoteDesktop, which
//! includes GNOME (xdg-desktop-portal-gnome) and KDE Plasma
//! (xdg-desktop-portal-kde).
//!
//! The portal API is async, so the session lives on a dedicated thread with
//! its own runtime and [`PortalBackend::inject`] just queues events for it.
//! That keeps `inject` cheap and callable from both sync and async code.
//!
//! When screen sharing is on, the same session also selects a monitor
//! through the ScreenCast interface. That's what lets the phone see the
//! screen, and what absolute pointer motion is relative to.

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::thread;

use ashpd::desktop::PersistMode;
use ashpd::desktop::Session;
use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::enumflags2::BitFlags;
use ferrokey_protocol::{InputEvent, MouseButton};
use tokio::sync::mpsc;

use crate::screen::Screen;
use crate::{Error, InputBackend};

// Linux evdev button codes, from linux/input-event-codes.h.
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;

pub struct PortalBackend {
    events: mpsc::UnboundedSender<InputEvent>,
}

impl PortalBackend {
    /// Starts a RemoteDesktop session, blocking until it is running, and
    /// returns the shared screen if `screen` is set and the user picked one.
    ///
    /// The first run shows the compositor's permission dialog. The restore
    /// token it hands back is saved so later runs can skip the dialog.
    pub fn new(screen: bool) -> Result<(Self, Option<Screen>), Error> {
        let (events, rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);

        thread::Builder::new()
            .name("ferrokey-portal".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(Error::backend(err)));
                        return;
                    }
                };
                runtime.block_on(async move {
                    match Portal::start(screen).await {
                        Ok(portal) => {
                            let screen = portal.shared.as_ref().map(|shared| shared.screen.clone());
                            let _ = ready_tx.send(Ok(screen));
                            portal.run(rx).await;
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(Error::backend(err)));
                        }
                    }
                });
            })
            .map_err(Error::backend)?;

        let screen = ready_rx.recv().map_err(|_| Error::SessionClosed)??;
        Ok((Self { events }, screen))
    }
}

impl InputBackend for PortalBackend {
    fn inject(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events.send(event).map_err(|_| Error::SessionClosed)
    }
}

struct Portal {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
    shared: Option<SharedScreen>,
}

/// The monitor shared through the session's ScreenCast stream.
struct SharedScreen {
    node_id: u32,
    /// The stream's size in the compositor's logical coordinates, which
    /// absolute pointer motion uses; may differ from its size in pixels.
    logical_size: Option<(i32, i32)>,
    screen: Screen,
}

impl Portal {
    async fn start(screen: bool) -> ashpd::Result<Self> {
        let proxy = RemoteDesktop::new().await?;
        let session = proxy.create_session(Default::default()).await?;

        let screencast = if screen {
            match select_screen(&session).await {
                Ok(screencast) => Some(screencast),
                Err(err) => {
                    eprintln!("ferrokey-input: screen sharing unavailable: {err}");
                    None
                }
            }
        } else {
            None
        };

        let token_file = if screencast.is_some() {
            SCREEN_TOKEN_FILE
        } else {
            INPUT_TOKEN_FILE
        };
        let restore_token = load_restore_token(token_file);
        proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token.as_deref()),
            )
            .await?;

        let selected = proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;
        if let Some(token) = selected.restore_token() {
            save_restore_token(token_file, token);
        }

        let shared = match (screencast, selected.streams().first()) {
            (Some(screencast), Some(stream)) => {
                match open_screen(&screencast, &session, stream.pipe_wire_node_id()).await {
                    Ok(screen) => Some(SharedScreen {
                        node_id: stream.pipe_wire_node_id(),
                        logical_size: stream.size(),
                        screen,
                    }),
                    Err(err) => {
                        eprintln!("ferrokey-input: screen sharing unavailable: {err}");
                        None
                    }
                }
            }
            (Some(_), None) => {
                eprintln!("ferrokey-input: screen sharing unavailable: no screen was shared");
                None
            }
            (None, _) => None,
        };

        Ok(Self {
            proxy,
            session,
            shared,
        })
    }

    /// Forwards queued events to the portal until every sender is dropped.
    async fn run(self, mut events: mpsc::UnboundedReceiver<InputEvent>) {
        while let Some(event) = events.recv().await {
            if let Err(err) = self.notify(event).await {
                eprintln!("ferrokey-input: failed to inject {event:?}: {err}");
            }
        }
        let _ = self.session.close().await;
    }

    async fn notify(&self, event: InputEvent) -> ashpd::Result<()> {
        let (proxy, session) = (&self.proxy, &self.session);
        match event {
            InputEvent::MouseMove { dx, dy } => {
                proxy
                    .notify_pointer_motion(session, dx.into(), dy.into(), Default::default())
                    .await
            }
            InputEvent::MouseButton { button, pressed } => {
                let button = match button {
                    MouseButton::Left => BTN_LEFT,
                    MouseButton::Right => BTN_RIGHT,
                    MouseButton::Middle => BTN_MIDDLE,
                };
                proxy
                    .notify_pointer_button(session, button, key_state(pressed), Default::default())
                    .await
            }
            InputEvent::Scroll { dx, dy } => {
                if dy != 0 {
                    proxy
                        .notify_pointer_axis_discrete(
                            session,
                            Axis::Vertical,
                            dy,
                            Default::default(),
                        )
                        .await?;
                }
                if dx != 0 {
                    proxy
                        .notify_pointer_axis_discrete(
                            session,
                            Axis::Horizontal,
                            dx,
                            Default::default(),
                        )
                        .await?;
                }
                Ok(())
            }
            InputEvent::Key { code, pressed } => {
                let code = i32::try_from(code)
                    .map_err(|_| ashpd::Error::ParseError("keycode out of range"))?;
                proxy
                    .notify_keyboard_keycode(session, code, key_state(pressed), Default::default())
                    .await
            }
            InputEvent::MouseMoveAbsolute { x, y } => {
                let Some(shared) = &self.shared else {
                    return Err(ashpd::Error::ParseError(
                        "absolute pointer motion needs screen sharing",
                    ));
                };
                let Some((width, height)) = shared
                    .logical_size
                    .map(|(w, h)| (f64::from(w), f64::from(h)))
                    .or_else(|| {
                        let (w, h) = shared.screen.frame_size()?;
                        Some((f64::from(w), f64::from(h)))
                    })
                else {
                    return Ok(());
                };
                let max = f64::from(InputEvent::ABSOLUTE_MAX);
                proxy
                    .notify_pointer_motion_absolute(
                        session,
                        shared.node_id,
                        f64::from(x) / max * width,
                        f64::from(y) / max * height,
                        Default::default(),
                    )
                    .await
            }
        }
    }
}

/// Asks for one monitor on the RemoteDesktop session. Must happen before
/// the session starts, since the permission dialog covers both.
async fn select_screen(session: &Session<RemoteDesktop>) -> ashpd::Result<Screencast> {
    let screencast = Screencast::new().await?;
    // Draw the pointer into the frames so the phone shows where it is.
    let cursor_mode = match screencast.available_cursor_modes().await {
        Ok(modes) if modes.contains(CursorMode::Embedded) => CursorMode::Embedded,
        _ => CursorMode::Hidden,
    };
    screencast
        .select_sources(
            session,
            SelectSourcesOptions::default()
                .set_sources(BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_cursor_mode(cursor_mode),
        )
        .await?;
    Ok(screencast)
}

#[cfg(feature = "screen")]
async fn open_screen(
    screencast: &Screencast,
    session: &Session<RemoteDesktop>,
    node_id: u32,
) -> Result<Screen, Error> {
    let fd = screencast
        .open_pipe_wire_remote(session, Default::default())
        .await
        .map_err(Error::backend)?;
    crate::pipewire::capture(fd, node_id)
}

#[cfg(not(feature = "screen"))]
async fn open_screen(
    _screencast: &Screencast,
    _session: &Session<RemoteDesktop>,
    _node_id: u32,
) -> Result<Screen, Error> {
    Err(Error::Unsupported)
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

/// Restore tokens cover what the dialog granted, so sessions with and
/// without the screen keep separate ones.
const INPUT_TOKEN_FILE: &str = "portal-restore-token";
const SCREEN_TOKEN_FILE: &str = "portal-restore-token-screen";

/// `$XDG_STATE_HOME/ferrokey/<file>`, falling back to `~/.local/state` as the
/// XDG base directory spec prescribes.
fn restore_token_path(file: &str) -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state_home.join("ferrokey").join(file))
}

fn load_restore_token(file: &str) -> Option<String> {
    let token = std::fs::read_to_string(restore_token_path(file)?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Saving is best effort: without a token the user just sees the dialog again.
fn save_restore_token(file: &str, token: &str) {
    let Some(path) = restore_token_path(file) else {
        return;
    };
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, token));
    if let Err(err) = result {
        eprintln!(
            "ferrokey-input: could not save portal restore token to {}: {err}",
            path.display()
        );
    }
}
