//! Linux backend built on the xdg-desktop-portal RemoteDesktop interface.
//!
//! Works on any compositor whose portal implements RemoteDesktop, which
//! includes GNOME (xdg-desktop-portal-gnome) and KDE Plasma
//! (xdg-desktop-portal-kde).
//!
//! Events go to the compositor either as one D-Bus call each ([`dbus`]), or,
//! with [`Options::eis`], over a libei connection the portal hands out
//! ([`eis`]). A portal session can't mix the two.
//!
//! When screen sharing is on, the same session also selects a monitor
//! through the ScreenCast interface, and asks for a touchscreen device.
//! That's what lets the phone see the screen, and what absolute pointer and
//! touch positions are relative to.
//!
//! The portal API is async, so the session lives on a dedicated thread with
//! its own runtime and [`PortalBackend::inject`] just queues events for it.
//! That keeps `inject` cheap and callable from both sync and async code.

mod dbus;
mod eis;
mod layout;

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::thread;

use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use ferrokey_protocol::InputEvent;
use tokio::sync::mpsc;

use crate::screen::Screen;
use crate::{Error, InputBackend, Options};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const NO_SCREEN: &str = "absolute positions need screen sharing, which is off or unavailable";

pub struct PortalBackend {
    events: mpsc::UnboundedSender<InputEvent>,
}

impl PortalBackend {
    /// Starts a RemoteDesktop session, blocking until it is running, and
    /// returns the shared screen if [`Options::screen`] is set and the user
    /// picked one.
    ///
    /// The first run shows the compositor's permission dialog. The restore
    /// token it hands back is saved so later runs can skip the dialog.
    pub fn new(options: Options) -> Result<(Self, Option<Screen>), Error> {
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
                    let portal = match Portal::start(options).await {
                        Ok(portal) => portal,
                        Err(err) => {
                            let _ = ready_tx.send(Err(Error::Backend(err)));
                            return;
                        }
                    };
                    let eis = if options.eis {
                        match portal.connect_to_eis().await {
                            Ok(eis) => Some(eis),
                            Err(err) => {
                                let _ = portal.session.close().await;
                                let _ = ready_tx.send(Err(Error::Backend(err)));
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    let screen = portal.shared.as_ref().map(|shared| shared.screen.clone());
                    let _ = ready_tx.send(Ok(screen));
                    match eis {
                        Some(eis) => eis.run(rx).await,
                        None => dbus::run(&portal, rx).await,
                    }
                    let _ = portal.session.close().await;
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

/// A running RemoteDesktop session.
struct Portal {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
    devices: BitFlags<DeviceType>,
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
    async fn start(options: Options) -> Result<Self, BoxError> {
        let proxy = RemoteDesktop::new().await?;
        if options.eis && proxy.version() < 2 {
            return Err(format!(
                "libei needs version 2 of the RemoteDesktop portal, but this one is version {}",
                proxy.version()
            )
            .into());
        }
        let session = proxy.create_session(Default::default()).await?;

        let screencast = if options.screen {
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

        // Touch positions are relative to the shared screen, so a
        // touchscreen is only useful with one.
        let mut devices = DeviceType::Keyboard | DeviceType::Pointer;
        if screencast.is_some() {
            match proxy.available_device_types().await {
                Ok(available) if available.contains(DeviceType::Touchscreen) => {
                    devices |= DeviceType::Touchscreen;
                }
                _ => eprintln!(
                    "ferrokey-input: the portal offers no touchscreen; touch input won't work"
                ),
            }
        }

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
                    .set_devices(devices)
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
            devices: selected.devices(),
            shared,
        })
    }

    async fn connect_to_eis(&self) -> Result<eis::Eis, BoxError> {
        let fd = self
            .proxy
            .connect_to_eis(&self.session, Default::default())
            .await?;
        eis::Eis::connect(UnixStream::from(fd)).await
    }

    /// Places an absolute position, in [`InputEvent::ABSOLUTE_MAX`] units,
    /// on the shared screen: its PipeWire node and the point in the stream's
    /// logical coordinates. `None` until the screen's size is known.
    fn screen_point(&self, x: u16, y: u16) -> Result<Option<(u32, f64, f64)>, InjectError> {
        let shared = self
            .shared
            .as_ref()
            .ok_or(InjectError::Unavailable(NO_SCREEN))?;
        let Some((width, height)) = shared
            .logical_size
            .map(|(w, h)| (f64::from(w), f64::from(h)))
            .or_else(|| {
                let (w, h) = shared.screen.frame_size()?;
                Some((f64::from(w), f64::from(h)))
            })
        else {
            return Ok(None);
        };
        let max = f64::from(InputEvent::ABSOLUTE_MAX);
        Ok(Some((
            shared.node_id,
            f64::from(x) / max * width,
            f64::from(y) / max * height,
        )))
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

/// Why an event wasn't injected.
enum InjectError {
    /// The session can't inject this kind of event at all. Reported once
    /// per reason, since every such event fails the same way.
    Unavailable(&'static str),
    /// Injecting this event failed.
    Failed(BoxError),
}

impl<E: Into<BoxError>> From<E> for InjectError {
    fn from(err: E) -> Self {
        InjectError::Failed(err.into())
    }
}

/// Prints injection errors, each [`InjectError::Unavailable`] only once.
#[derive(Default)]
struct Reporter {
    reported: HashSet<&'static str>,
}

impl Reporter {
    fn report(&mut self, event: InputEvent, result: Result<(), InjectError>) {
        match result {
            Ok(()) => {}
            Err(InjectError::Unavailable(reason)) => {
                if self.reported.insert(reason) {
                    eprintln!("ferrokey-input: skipping events like {event:?}: {reason}");
                }
            }
            Err(InjectError::Failed(err)) => {
                eprintln!("ferrokey-input: failed to inject {event:?}: {err}");
            }
        }
    }
}

/// Restore tokens cover what the dialog granted, so sessions with and
/// without the screen keep separate ones.
const INPUT_TOKEN_FILE: &str = "portal-restore-token";
const SCREEN_TOKEN_FILE: &str = "portal-restore-token-screen";

/// `$XDG_STATE_HOME/ferrokey/<name>`, falling back to
/// `~/.local/state` as the XDG base directory spec prescribes.
fn restore_token_path(name: &str) -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state_home.join("ferrokey").join(name))
}

fn load_restore_token(name: &str) -> Option<String> {
    let token = std::fs::read_to_string(restore_token_path(name)?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Saving is best effort: without a token the user just sees the dialog again.
fn save_restore_token(name: &str, token: &str) {
    let Some(path) = restore_token_path(name) else {
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
