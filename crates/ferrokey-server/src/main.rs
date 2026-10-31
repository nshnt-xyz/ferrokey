//! Runs on the controlled machine, injects input events from paired
//! clients and shows them the screen.

mod encode;
mod store;

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use ferrokey_input::InputBackend;
use ferrokey_input::screen::Screen;
use ferrokey_net::server::{ClientMessage, Features};
use ferrokey_net::{DEFAULT_PORT, FRAMES_IN_FLIGHT, Keypair, server};
use ferrokey_protocol::{InputEvent, MouseButton};

use crate::encode::FrameEncoder;
use crate::store::Store;

const USAGE: &str = "\
Usage: ferrokey-server [OPTIONS]

Options:
  --listen <ADDR>    Address to listen on [default: 0.0.0.0:47800]
  --pair             Show a pairing code so a new device can pair
  --print-events     Print events instead of injecting them
  --no-screen        Don't offer to share the screen
  --eis              Send input through libei instead of D-Bus calls (Linux,
                     needs RemoteDesktop portal version 2)
  --state-dir <DIR>  Where the server key and paired devices are kept
  -h, --help         Print this help
";

struct Args {
    listen: SocketAddr,
    pair: bool,
    print_events: bool,
    screen: bool,
    eis: bool,
    state_dir: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
        pair: false,
        print_events: false,
        screen: true,
        eis: false,
        state_dir: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| iter.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--listen" => {
                let value = value("--listen")?;
                args.listen = value
                    .parse()
                    .map_err(|_| format!("invalid address: {value}"))?;
            }
            "--pair" => args.pair = true,
            "--print-events" => args.print_events = true,
            "--no-screen" => args.screen = false,
            "--eis" => args.eis = true,
            "--state-dir" => args.state_dir = Some(value("--state-dir")?.into()),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("ferrokey-server: {err}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ferrokey-server: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = args
        .state_dir
        .or_else(store::default_dir)
        .ok_or("no state directory; pass --state-dir")?;
    let keypair = store::load_or_create_keypair(&state_dir)?;
    let store = Arc::new(Store::open(&state_dir)?);

    let (backend, screen): (Box<dyn InputBackend>, _) = if args.print_events {
        (Box::new(PrintBackend), None)
    } else {
        let backend = ferrokey_input::backend(ferrokey_input::Options {
            screen: args.screen,
            eis: args.eis,
        })?;
        (backend.input, backend.screen)
    };
    let backend = Arc::new(Mutex::new(backend));
    if screen.is_some() {
        println!("Screen sharing is available");
    }

    let listener = TcpListener::bind(args.listen)?;
    println!("Listening on {}", listener.local_addr()?);
    println!("Server fingerprint: {}", keypair.public().fingerprint());
    if args.pair || store.device_count() == 0 {
        let code = store.start_pairing()?;
        println!("Pairing code: {} {}", &code[..3], &code[3..]);
    }

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("accept failed: {err}");
                continue;
            }
        };
        let (keypair, store, backend, screen) = (
            keypair.clone(),
            store.clone(),
            backend.clone(),
            screen.clone(),
        );
        thread::spawn(move || handle(stream, &keypair, &store, &backend, screen));
    }
    Ok(())
}

fn handle(
    stream: TcpStream,
    keypair: &Keypair,
    store: &Store,
    backend: &Mutex<Box<dyn InputBackend>>,
    screen: Option<Screen>,
) {
    let peer = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_owned(), |addr| addr.to_string());
    let features = Features {
        screen: screen.is_some(),
    };
    let session = match server::accept(stream, keypair, store, features) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{peer}: handshake failed: {err}");
            return;
        }
    };
    let device = format!("{} ({peer})", session.device_name());
    if session.newly_paired() {
        println!(
            "Paired {device}, fingerprint {}",
            session.device_key().fingerprint()
        );
    }
    println!("{device} connected");

    let (sender, mut receiver) = session.split();
    let request = Arc::new(ScreenRequest::default());
    let streamer = thread::spawn({
        let (request, device) = (request.clone(), device.clone());
        move || stream_screen(sender, screen, &request, &device)
    });

    let mut held = Held::default();
    let result = loop {
        let events = match receiver.recv() {
            Ok(ClientMessage::Input(events)) => events,
            Ok(ClientMessage::Keepalive) => continue,
            Ok(message) => {
                request.update(message);
                continue;
            }
            Err(err) => break err,
        };
        let mut backend = backend.lock().unwrap();
        for event in events {
            held.track(event);
            if let Err(err) = backend.inject(event) {
                eprintln!("{device}: failed to inject {event:?}: {err}");
            }
        }
    };
    // Don't leave a button, key or finger down if the client vanished
    // mid-press.
    let mut backend = backend.lock().unwrap();
    for event in held.releases() {
        let _ = backend.inject(event);
    }
    drop(backend);
    // Unblock a frame write, then let the streamer see the session is over.
    receiver.shutdown();
    request.close();
    let _ = streamer.join();
    match result {
        ferrokey_net::Error::Io(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            println!("{device} disconnected");
        }
        err => println!("{device} disconnected: {err}"),
    }
}

/// What a client wants from the screen, passed from the thread receiving
/// its messages to the thread sending it frames.
#[derive(Default)]
struct ScreenRequest {
    state: Mutex<RequestState>,
    changed: Condvar,
}

#[derive(Default)]
struct RequestState {
    /// The size frames must fit in, while the client watches.
    watching: Option<(u16, u16)>,
    /// Frames sent but not yet acknowledged.
    in_flight: u32,
    /// The connection is gone.
    closed: bool,
}

/// What the streaming thread should do next.
enum Next {
    /// Send a frame of at most this size.
    Send(u16, u16),
    /// The client stopped watching.
    Stop,
    /// The connection closed.
    Close,
}

impl ScreenRequest {
    fn update(&self, message: ClientMessage) {
        let mut state = self.state.lock().unwrap();
        match message {
            ClientMessage::StartScreen {
                max_width,
                max_height,
            } => {
                state.watching = Some((max_width, max_height));
                state.in_flight = 0;
            }
            ClientMessage::StopScreen => state.watching = None,
            ClientMessage::FrameAck => state.in_flight = state.in_flight.saturating_sub(1),
            ClientMessage::Keepalive | ClientMessage::Input(_) => return,
        }
        self.changed.notify_all();
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.changed.notify_all();
    }

    /// Waits until the client is watching, returning false once the
    /// connection closes.
    fn wait_for_start(&self) -> bool {
        let state = self.state.lock().unwrap();
        let state = self
            .changed
            .wait_while(state, |state| !state.closed && state.watching.is_none())
            .unwrap();
        !state.closed
    }

    /// Waits until another frame may be sent, and reserves its place.
    fn wait_to_send(&self) -> Next {
        let mut state = self.state.lock().unwrap();
        loop {
            match *state {
                RequestState { closed: true, .. } => return Next::Close,
                RequestState { watching: None, .. } => return Next::Stop,
                RequestState {
                    watching: Some((width, height)),
                    in_flight,
                    ..
                } if in_flight < FRAMES_IN_FLIGHT => return Next::Send(width, height),
                _ => state = self.changed.wait(state).unwrap(),
            }
        }
    }

    /// Counts a frame as sent. Done before sending, so its acknowledgement
    /// can't arrive first.
    fn frame_sent(&self) {
        self.state.lock().unwrap().in_flight += 1;
    }

    /// Notes that the screen went away while the client was watching.
    fn stop(&self) {
        self.state.lock().unwrap().watching = None;
    }
}

/// Sends a client screen frames for as long as it watches, until the
/// connection closes.
fn stream_screen(
    mut sender: server::Sender,
    screen: Option<Screen>,
    request: &ScreenRequest,
    device: &str,
) {
    // How often to look for a stop request while the screen is unchanged.
    const POLL: Duration = Duration::from_millis(100);

    let mut encoder = FrameEncoder::default();
    while request.wait_for_start() {
        let Some(screen) = &screen else {
            request.stop();
            if sender.send_screen_stopped().is_err() {
                return;
            }
            continue;
        };
        println!("{device} is watching the screen");
        let mut viewer = screen.watch();
        loop {
            let (max_width, max_height) = match request.wait_to_send() {
                Next::Send(width, height) => (width, height),
                Next::Stop => break,
                Next::Close => return,
            };
            let frame = match viewer.next_frame(POLL) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(_) => {
                    println!("Screen sharing ended");
                    request.stop();
                    if sender.send_screen_stopped().is_err() {
                        return;
                    }
                    break;
                }
            };
            let (width, height) = match encoder.encode(&frame, max_width, max_height) {
                Ok(size) => size,
                Err(err) => {
                    eprintln!("{device}: failed to encode a frame: {err}");
                    continue;
                }
            };
            request.frame_sent();
            if sender.send_frame(width, height, &encoder.jpeg).is_err() {
                sender.shutdown();
                return;
            }
        }
        println!("{device} stopped watching the screen");
    }
}

/// Buttons, keys and fingers a client currently holds down.
#[derive(Default)]
struct Held {
    buttons: HashSet<MouseButton>,
    keys: HashSet<u32>,
    keysyms: HashSet<u32>,
    touches: HashSet<u32>,
}

impl Held {
    fn track(&mut self, event: InputEvent) {
        match event {
            InputEvent::MouseButton {
                button,
                pressed: true,
            } => {
                self.buttons.insert(button);
            }
            InputEvent::MouseButton {
                button,
                pressed: false,
            } => {
                self.buttons.remove(&button);
            }
            InputEvent::Key {
                code,
                pressed: true,
            } => {
                self.keys.insert(code);
            }
            InputEvent::Key {
                code,
                pressed: false,
            } => {
                self.keys.remove(&code);
            }
            InputEvent::Keysym {
                keysym,
                pressed: true,
            } => {
                self.keysyms.insert(keysym);
            }
            InputEvent::Keysym {
                keysym,
                pressed: false,
            } => {
                self.keysyms.remove(&keysym);
            }
            InputEvent::TouchDown { id, .. } => {
                self.touches.insert(id);
            }
            InputEvent::TouchUp { id } => {
                self.touches.remove(&id);
            }
            InputEvent::MouseMove { .. }
            | InputEvent::MouseMoveAbsolute { .. }
            | InputEvent::Scroll { .. }
            | InputEvent::SmoothScroll { .. }
            | InputEvent::TouchMove { .. } => {}
        }
    }

    fn releases(&self) -> impl Iterator<Item = InputEvent> + '_ {
        let buttons = self.buttons.iter().map(|&button| InputEvent::MouseButton {
            button,
            pressed: false,
        });
        let keys = self.keys.iter().map(|&code| InputEvent::Key {
            code,
            pressed: false,
        });
        let keysyms = self.keysyms.iter().map(|&keysym| InputEvent::Keysym {
            keysym,
            pressed: false,
        });
        let touches = self.touches.iter().map(|&id| InputEvent::TouchUp { id });
        buttons.chain(keys).chain(keysyms).chain(touches)
    }
}

/// Stands in for a real backend when testing clients.
struct PrintBackend;

impl InputBackend for PrintBackend {
    fn inject(&mut self, event: InputEvent) -> Result<(), ferrokey_input::Error> {
        println!("{event:?}");
        Ok(())
    }
}
