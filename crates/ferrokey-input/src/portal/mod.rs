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
//! With [`Options::screen`] the session also shares the screens through the
//! ScreenCast portal. Absolute pointer and touch positions are given
//! relative to a shared screen, so they need it.
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
use ashpd::desktop::screencast::{Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use ferrokey_protocol::InputEvent;
use tokio::sync::mpsc;

use self::layout::{Area, Layout};
use crate::{Error, InputBackend, Options};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct PortalBackend {
    events: mpsc::UnboundedSender<InputEvent>,
}

impl PortalBackend {
    /// Starts a RemoteDesktop session, blocking until it is running.
    ///
    /// The first run shows the compositor's permission dialog. The restore
    /// token it hands back is saved so later runs can skip the dialog.
    pub fn new(options: Options) -> Result<Self, Error> {
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
                    let _ = ready_tx.send(Ok(()));
                    match eis {
                        Some(eis) => eis.run(rx).await,
                        None => dbus::run(&portal, rx).await,
                    }
                    let _ = portal.session.close().await;
                });
            })
            .map_err(Error::backend)?;

        ready_rx.recv().map_err(|_| Error::SessionClosed)??;
        Ok(Self { events })
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
    devices: ashpd::enumflags2::BitFlags<DeviceType>,
    /// The shared screens, by PipeWire node id. Empty without
    /// [`Options::screen`].
    screens: Layout<u32>,
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

        let mut devices = DeviceType::Keyboard | DeviceType::Pointer;
        if options.screen {
            if proxy
                .available_device_types()
                .await?
                .contains(DeviceType::Touchscreen)
            {
                devices |= DeviceType::Touchscreen;
            } else {
                eprintln!(
                    "ferrokey-input: the portal offers no touchscreen; touch input won't work"
                );
            }
        }

        let session = proxy.create_session(Default::default()).await?;
        // A token only restores the same kind of session, so sessions with
        // and without a shared screen keep separate ones.
        let token_name = if options.screen {
            "portal-restore-token-screen"
        } else {
            "portal-restore-token"
        };
        let restore_token = load_restore_token(token_name);
        proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(devices)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token.as_deref()),
            )
            .await?;
        if options.screen {
            // Persistence is handled by the RemoteDesktop restore token.
            Screencast::new()
                .await?
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                        .set_multiple(true),
                )
                .await?;
        }

        let selected = proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;
        if let Some(token) = selected.restore_token() {
            save_restore_token(token_name, token);
        }

        let screens = Layout::new(selected.streams().iter().filter_map(|stream| {
            let (width, height) = stream.size()?;
            // Only a single stream may leave out its position.
            let (x, y) = stream.position().unwrap_or_default();
            Some(Area {
                id: stream.pipe_wire_node_id(),
                x: x.into(),
                y: y.into(),
                width: width.into(),
                height: height.into(),
            })
        }));
        if options.screen && screens.is_empty() {
            eprintln!(
                "ferrokey-input: no screen with a known size was shared; \
                 absolute pointer and touch input won't work"
            );
        }

        Ok(Self {
            proxy,
            session,
            devices: selected.devices(),
            screens,
        })
    }

    async fn connect_to_eis(&self) -> Result<eis::Eis, BoxError> {
        let fd = self
            .proxy
            .connect_to_eis(&self.session, Default::default())
            .await?;
        eis::Eis::connect(UnixStream::from(fd)).await
    }
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
