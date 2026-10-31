//! Screen capture, shared between however many clients are watching.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::Error;

/// One captured frame, in the capture source's own pixel layout.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub stride: usize,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

/// Four bytes per pixel, in memory order; `x` is padding or alpha, which is
/// ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx,
    Rgbx,
}

impl Frame {
    /// The red, green and blue byte offsets within a pixel.
    pub fn channel_offsets(&self) -> [usize; 3] {
        match self.format {
            PixelFormat::Bgrx => [2, 1, 0],
            PixelFormat::Rgbx => [0, 1, 2],
        }
    }
}

/// The local screen, as the capture backend sees it. Cheap to clone.
///
/// Capturing only runs while at least one [`Viewer`] exists.
#[derive(Clone)]
pub struct Screen {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Told when capture should start (`true`) or stop (`false`).
    set_active: Box<dyn Fn(bool) + Send + Sync>,
}

#[derive(Default)]
struct State {
    frame: Option<Arc<Frame>>,
    /// Counts published frames, so viewers can tell a new one from the last.
    serial: u64,
    viewers: usize,
    closed: bool,
}

impl Screen {
    #[cfg_attr(not(all(target_os = "linux", feature = "screen")), allow(dead_code))]
    pub(crate) fn new(set_active: impl Fn(bool) + Send + Sync + 'static) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::default(),
                changed: Condvar::new(),
                set_active: Box::new(set_active),
            }),
        }
    }

    /// Starts watching the screen. Frames come from [`Viewer::next_frame`].
    pub fn watch(&self) -> Viewer {
        let mut state = self.shared.state.lock().unwrap();
        state.viewers += 1;
        if state.viewers == 1 && !state.closed {
            (self.shared.set_active)(true);
        }
        Viewer {
            screen: self.clone(),
            seen: 0,
        }
    }

    /// Whether anyone is watching, i.e. whether frames are wanted.
    #[cfg_attr(not(all(target_os = "linux", feature = "screen")), allow(dead_code))]
    pub(crate) fn is_watched(&self) -> bool {
        self.shared.state.lock().unwrap().viewers > 0
    }

    /// Size of the latest frame, if there has been one.
    #[cfg_attr(not(all(target_os = "linux", feature = "screen")), allow(dead_code))]
    pub(crate) fn frame_size(&self) -> Option<(u32, u32)> {
        let state = self.shared.state.lock().unwrap();
        state
            .frame
            .as_ref()
            .map(|frame| (frame.width, frame.height))
    }

    /// Hands a new frame to every viewer.
    #[cfg_attr(not(all(target_os = "linux", feature = "screen")), allow(dead_code))]
    pub(crate) fn publish(&self, frame: Frame) {
        let mut state = self.shared.state.lock().unwrap();
        state.frame = Some(Arc::new(frame));
        state.serial += 1;
        self.shared.changed.notify_all();
    }

    /// Marks the capture as over for good, e.g. because the user stopped
    /// sharing from the desktop.
    #[cfg_attr(not(all(target_os = "linux", feature = "screen")), allow(dead_code))]
    pub(crate) fn close(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.closed = true;
        state.frame = None;
        self.shared.changed.notify_all();
    }
}

/// Someone watching the [`Screen`].
pub struct Viewer {
    screen: Screen,
    /// Serial of the last frame returned.
    seen: u64,
}

impl Viewer {
    /// Waits up to `timeout` for a frame newer than the last one returned,
    /// starting with the latest one already captured.
    ///
    /// Frames that arrive faster than they are asked for are skipped, so a
    /// slow viewer always gets the latest. Fails once capture has ended.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<Frame>>, Error> {
        let shared = &self.screen.shared;
        let deadline = Instant::now() + timeout;
        let mut state = shared.state.lock().unwrap();
        loop {
            if state.closed {
                return Err(Error::SessionClosed);
            }
            if state.serial != self.seen
                && let Some(frame) = &state.frame
            {
                self.seen = state.serial;
                return Ok(Some(frame.clone()));
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(None);
            };
            state = shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let shared = &self.screen.shared;
        let mut state = shared.state.lock().unwrap();
        state.viewers -= 1;
        if state.viewers == 0 && !state.closed {
            (shared.set_active)(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn frame(width: u32) -> Frame {
        Frame {
            width,
            height: 1,
            stride: width as usize * 4,
            format: PixelFormat::Bgrx,
            data: vec![0; width as usize * 4],
        }
    }

    #[test]
    fn viewers_get_the_latest_frame_and_toggle_capture() {
        let active = Arc::new(AtomicBool::new(false));
        let flag = active.clone();
        let screen = Screen::new(move |on| flag.store(on, Ordering::SeqCst));
        let short = Duration::from_millis(10);

        let mut first = screen.watch();
        let second = screen.watch();
        assert!(active.load(Ordering::SeqCst));
        assert!(first.next_frame(short).unwrap().is_none());

        screen.publish(frame(1));
        screen.publish(frame(2));
        assert_eq!(first.next_frame(short).unwrap().unwrap().width, 2);
        assert!(first.next_frame(short).unwrap().is_none());
        assert_eq!(screen.frame_size(), Some((2, 1)));

        drop(second);
        assert!(active.load(Ordering::SeqCst));
        drop(first);
        assert!(!active.load(Ordering::SeqCst));
        assert!(!screen.is_watched());

        // A new viewer starts from the frame already captured.
        let mut third = screen.watch();
        assert_eq!(third.next_frame(short).unwrap().unwrap().width, 2);
        screen.close();
        assert!(third.next_frame(short).is_err());
    }
}
