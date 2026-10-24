//! Linux backend built on the xdg-desktop-portal RemoteDesktop interface.
//!
//! Works on any compositor whose portal implements RemoteDesktop, which
//! includes GNOME (xdg-desktop-portal-gnome) and KDE Plasma
//! (xdg-desktop-portal-kde).
//!
//! The portal API is async, so the session lives on a dedicated thread with
//! its own runtime and [`PortalBackend::inject`] just queues events for it.
//! That keeps `inject` cheap and callable from both sync and async code.

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::thread;

use ashpd::desktop::PersistMode;
use ashpd::desktop::Session;
use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
};
use ferrokey_protocol::{InputEvent, MouseButton};
use tokio::sync::mpsc;

use crate::{Error, InputBackend};

// Linux evdev button codes, from linux/input-event-codes.h.
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;

pub struct PortalBackend {
    events: mpsc::UnboundedSender<InputEvent>,
}

impl PortalBackend {
    /// Starts a RemoteDesktop session, blocking until it is running.
    ///
    /// The first run shows the compositor's permission dialog. The restore
    /// token it hands back is saved so later runs can skip the dialog.
    pub fn new() -> Result<Self, Error> {
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
                    match Portal::start().await {
                        Ok(portal) => {
                            let _ = ready_tx.send(Ok(()));
                            portal.run(rx).await;
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(Error::backend(err)));
                        }
                    }
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

struct Portal {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

impl Portal {
    async fn start() -> ashpd::Result<Self> {
        let proxy = RemoteDesktop::new().await?;
        let session = proxy.create_session(Default::default()).await?;

        let restore_token = load_restore_token();
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
            save_restore_token(token);
        }

        Ok(Self { proxy, session })
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
        }
    }
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

/// `$XDG_STATE_HOME/ferrokey/portal-restore-token`, falling back to
/// `~/.local/state` as the XDG base directory spec prescribes.
fn restore_token_path() -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state_home.join("ferrokey/portal-restore-token"))
}

fn load_restore_token() -> Option<String> {
    let token = std::fs::read_to_string(restore_token_path()?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Saving is best effort: without a token the user just sees the dialog again.
fn save_restore_token(token: &str) {
    let Some(path) = restore_token_path() else {
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
