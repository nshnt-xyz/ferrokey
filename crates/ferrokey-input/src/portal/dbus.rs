//! Sends each event to the portal as a D-Bus call.

use ashpd::desktop::remote_desktop::{Axis, DeviceType, KeyState, NotifyPointerAxisOptions};
use ferrokey_protocol::{InputEvent, MouseButton};
use tokio::sync::mpsc;

use super::{InjectError, Portal, Reporter};

// Linux evdev button codes, from linux/input-event-codes.h.
pub(super) const BTN_LEFT: u32 = 0x110;
pub(super) const BTN_RIGHT: u32 = 0x111;
pub(super) const BTN_MIDDLE: u32 = 0x112;

pub(super) const NO_SCREEN: &str =
    "absolute positions need a shared screen; start the server with --screen";
pub(super) const NO_TOUCHSCREEN: &str =
    "the session has no touchscreen; start the server with --screen";

pub(super) fn button_code(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
    }
}

/// Forwards queued events to the portal until every sender is dropped.
pub(super) async fn run(portal: &Portal, mut events: mpsc::UnboundedReceiver<InputEvent>) {
    let mut reporter = Reporter::default();
    while let Some(event) = events.recv().await {
        reporter.report(event, notify(portal, event).await);
    }
}

async fn notify(portal: &Portal, event: InputEvent) -> Result<(), InjectError> {
    let (proxy, session) = (&portal.proxy, &portal.session);
    match event {
        InputEvent::MouseMove { dx, dy } => {
            proxy
                .notify_pointer_motion(session, dx.into(), dy.into(), Default::default())
                .await?;
        }
        InputEvent::MouseButton { button, pressed } => {
            let button = i32::try_from(button_code(button))?;
            proxy
                .notify_pointer_button(session, button, key_state(pressed), Default::default())
                .await?;
        }
        InputEvent::Scroll { dx, dy } => {
            if dy != 0 {
                proxy
                    .notify_pointer_axis_discrete(session, Axis::Vertical, dy, Default::default())
                    .await?;
            }
            if dx != 0 {
                proxy
                    .notify_pointer_axis_discrete(session, Axis::Horizontal, dx, Default::default())
                    .await?;
            }
        }
        InputEvent::SmoothScroll { dx, dy, finish } => {
            proxy
                .notify_pointer_axis(
                    session,
                    dx.into(),
                    dy.into(),
                    NotifyPointerAxisOptions::default().set_finish(finish),
                )
                .await?;
        }
        InputEvent::Key { code, pressed } => {
            let code = i32::try_from(code)?;
            proxy
                .notify_keyboard_keycode(session, code, key_state(pressed), Default::default())
                .await?;
        }
        InputEvent::Keysym { keysym, pressed } => {
            let keysym = i32::try_from(keysym)?;
            proxy
                .notify_keyboard_keysym(session, keysym, key_state(pressed), Default::default())
                .await?;
        }
        InputEvent::MoveTo { x, y } => {
            let point = portal
                .screens
                .locate(x, y)
                .ok_or(InjectError::Unavailable(NO_SCREEN))?;
            let (x, y) = point.local;
            proxy
                .notify_pointer_motion_absolute(session, point.id, x, y, Default::default())
                .await?;
        }
        InputEvent::TouchDown { id, x, y } | InputEvent::TouchMove { id, x, y } => {
            if !portal.devices.contains(DeviceType::Touchscreen) {
                return Err(InjectError::Unavailable(NO_TOUCHSCREEN));
            }
            let point = portal
                .screens
                .locate(x, y)
                .ok_or(InjectError::Unavailable(NO_SCREEN))?;
            let (x, y) = point.local;
            if matches!(event, InputEvent::TouchDown { .. }) {
                proxy
                    .notify_touch_down(session, point.id, id, x, y, Default::default())
                    .await?;
            } else {
                proxy
                    .notify_touch_motion(session, point.id, id, x, y, Default::default())
                    .await?;
            }
        }
        InputEvent::TouchUp { id } => {
            if !portal.devices.contains(DeviceType::Touchscreen) {
                return Err(InjectError::Unavailable(NO_TOUCHSCREEN));
            }
            proxy
                .notify_touch_up(session, id, Default::default())
                .await?;
        }
    }
    Ok(())
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}
