//! Reads frames from a ScreenCast portal stream through PipeWire.
//!
//! PipeWire objects can't leave the thread that made them, so the stream
//! lives on its own thread running a PipeWire main loop. [`Screen`] talks to
//! it through a PipeWire channel, to pause the stream while nobody watches.

use std::cell::Cell;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::mpsc as std_mpsc;
use std::thread;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::ParamType;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Id, Rectangle, SpaTypes};

use crate::Error;
use crate::screen::{Frame, PixelFormat, Screen};

/// Connects to the PipeWire remote `fd` from the portal and captures stream
/// `node_id`, returning once the stream is set up.
pub(crate) fn capture(fd: OwnedFd, node_id: u32) -> Result<Screen, Error> {
    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    thread::Builder::new()
        .name("ferrokey-pipewire".into())
        .spawn(move || {
            if let Err(err) = run(fd, node_id, &ready_tx) {
                let _ = ready_tx.send(Err(Error::backend(err)));
            }
        })
        .map_err(Error::backend)?;
    ready_rx.recv().map_err(|_| Error::SessionClosed)?
}

/// What the stream agreed on, filled in once negotiation finishes.
#[derive(Clone, Copy)]
struct Negotiated {
    width: u32,
    height: u32,
    format: PixelFormat,
}

fn run(
    fd: OwnedFd,
    node_id: u32,
    ready: &std_mpsc::SyncSender<Result<Screen, Error>>,
) -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;

    let (active_tx, active_rx) = pw::channel::channel::<bool>();
    let active_tx = Mutex::new(active_tx);
    let screen = Screen::new(move |active| {
        let _ = active_tx.lock().unwrap().send(active);
    });

    let stream = pw::stream::StreamRc::new(
        core,
        "ferrokey-screen",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let negotiated: Rc<Cell<Option<Negotiated>>> = Rc::default();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed({
            let (screen, mainloop) = (screen.clone(), mainloop.clone());
            move |_, _, _, new| {
                if let pw::stream::StreamState::Error(err) = new {
                    eprintln!("ferrokey-input: screen capture failed: {err}");
                    screen.close();
                    mainloop.quit();
                }
            }
        })
        .param_changed({
            let negotiated = negotiated.clone();
            move |stream, _, id, param| {
                let Some(param) = param else { return };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                negotiated.set(parse_format(param));
                // Ask for buffers in ordinary memory, which can be read
                // directly, rather than GPU buffers.
                if let Some(buffers) = serialize(buffers_param()) {
                    let Some(pod) = Pod::from_bytes(&buffers) else {
                        return;
                    };
                    let _ = stream.update_params(&mut [pod]);
                }
            }
        })
        .process({
            let (screen, negotiated) = (screen.clone(), negotiated.clone());
            move |stream, _| {
                // Always dequeue, so the buffer goes back to the compositor.
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(negotiated) = negotiated.get() else {
                    return;
                };
                if !screen.is_watched() {
                    return;
                }
                if let Some(data) = buffer.datas_mut().first_mut()
                    && let Some(frame) = copy_frame(data, negotiated)
                {
                    screen.publish(frame);
                }
            }
        })
        .register()?;

    let formats = serialize(format_param()).ok_or(pw::Error::CreationFailed)?;
    let mut params = [Pod::from_bytes(&formats).ok_or(pw::Error::CreationFailed)?];
    stream.connect(
        Direction::Input,
        Some(node_id),
        // Inactive until someone watches.
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::INACTIVE,
        &mut params,
    )?;

    let _active = active_rx.attach(mainloop.loop_(), {
        let stream = stream.clone();
        move |active| {
            if let Err(err) = stream.set_active(active) {
                eprintln!("ferrokey-input: could not pause or resume screen capture: {err}");
            }
        }
    });

    let _ = ready.send(Ok(screen.clone()));
    mainloop.run();
    screen.close();
    Ok(())
}

/// Copies the visible pixels out of a buffer, dropping row padding.
fn copy_frame(data: &mut spa::buffer::Data, negotiated: Negotiated) -> Option<Frame> {
    let chunk = data.chunk();
    if chunk.size() == 0 || chunk.flags().contains(spa::buffer::ChunkFlags::CORRUPTED) {
        return None;
    }
    let (width, height) = (negotiated.width as usize, negotiated.height as usize);
    let row_len = width * 4;
    let stride = usize::try_from(chunk.stride())
        .ok()
        .filter(|&stride| stride >= row_len)
        .unwrap_or(row_len);
    let offset = chunk.offset() as usize;

    let bytes = data.data()?;
    let end = offset + stride * height.checked_sub(1)? + row_len;
    if width == 0 || bytes.len() < end {
        return None;
    }
    let mut pixels = Vec::with_capacity(row_len * height);
    for row in bytes[offset..end].chunks(stride) {
        pixels.extend_from_slice(&row[..row_len]);
    }
    Some(Frame {
        width: negotiated.width,
        height: negotiated.height,
        stride: row_len,
        format: negotiated.format,
        data: pixels,
    })
}

fn parse_format(param: &Pod) -> Option<Negotiated> {
    let (media_type, media_subtype) = spa::param::format_utils::parse_format(param).ok()?;
    if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
        return None;
    }
    let mut info = VideoInfoRaw::default();
    info.parse(param).ok()?;
    let format = match info.format() {
        VideoFormat::BGRx | VideoFormat::BGRA => PixelFormat::Bgrx,
        VideoFormat::RGBx | VideoFormat::RGBA => PixelFormat::Rgbx,
        _ => return None,
    };
    let size = info.size();
    Some(Negotiated {
        width: size.width,
        height: size.height,
        format,
    })
}

/// The formats we can read: 32-bit RGB in either byte order, any size and
/// frame rate.
fn format_param() -> Object {
    let id = |id: u32| Value::Id(Id(id));
    let choice = Value::Choice;
    Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                id(MediaType::Video.as_raw()),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                id(MediaSubtype::Raw.as_raw()),
            ),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: Id(VideoFormat::BGRx.as_raw()),
                        alternatives: [
                            VideoFormat::BGRx,
                            VideoFormat::BGRA,
                            VideoFormat::RGBx,
                            VideoFormat::RGBA,
                        ]
                        .map(|format| Id(format.as_raw()))
                        .to_vec(),
                    },
                ))),
            ),
            Property::new(
                FormatProperties::VideoSize.as_raw(),
                choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Rectangle {
                            width: 1920,
                            height: 1080,
                        },
                        min: Rectangle {
                            width: 1,
                            height: 1,
                        },
                        max: Rectangle {
                            width: 8192,
                            height: 8192,
                        },
                    },
                ))),
            ),
            Property::new(
                FormatProperties::VideoFramerate.as_raw(),
                choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Fraction { num: 30, denom: 1 },
                        min: Fraction { num: 0, denom: 1 },
                        max: Fraction { num: 60, denom: 1 },
                    },
                ))),
            ),
        ],
    }
}

/// Buffers in memory we can map, as opposed to DMA-BUFs.
fn buffers_param() -> Object {
    let data_types = (1 << spa::sys::SPA_DATA_MemPtr) | (1 << spa::sys::SPA_DATA_MemFd);
    Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Int(data_types),
        )],
    }
}

fn serialize(object: Object) -> Option<Vec<u8>> {
    let (cursor, _) =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).ok()?;
    Some(cursor.into_inner())
}
