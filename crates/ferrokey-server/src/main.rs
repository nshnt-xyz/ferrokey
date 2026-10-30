//! Runs on the controlled machine and injects input events from paired
//! clients.

mod store;

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;

use ferrokey_input::InputBackend;
use ferrokey_net::{DEFAULT_PORT, Keypair, server};
use ferrokey_protocol::{InputEvent, MouseButton};

use crate::store::Store;

const USAGE: &str = "\
Usage: ferrokey-server [OPTIONS]

Options:
  --listen <ADDR>    Address to listen on [default: 0.0.0.0:47800]
  --pair             Show a pairing code so a new device can pair
  --print-events     Print events instead of injecting them
  --state-dir <DIR>  Where the server key and paired devices are kept
  -h, --help         Print this help
";

struct Args {
    listen: SocketAddr,
    pair: bool,
    print_events: bool,
    state_dir: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
        pair: false,
        print_events: false,
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

    let backend: Box<dyn InputBackend> = if args.print_events {
        Box::new(PrintBackend)
    } else {
        ferrokey_input::backend()?
    };
    let backend = Arc::new(Mutex::new(backend));

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
        let (keypair, store, backend) = (keypair.clone(), store.clone(), backend.clone());
        thread::spawn(move || handle(stream, &keypair, &store, &backend));
    }
    Ok(())
}

fn handle(
    stream: TcpStream,
    keypair: &Keypair,
    store: &Store,
    backend: &Mutex<Box<dyn InputBackend>>,
) {
    let peer = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_owned(), |addr| addr.to_string());
    let mut session = match server::accept(stream, keypair, store) {
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

    let mut held = Held::default();
    let result = loop {
        let events = match session.recv_events() {
            Ok(events) => events,
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
    // Don't leave a button or key stuck down if the client vanished mid-press.
    let mut backend = backend.lock().unwrap();
    for event in held.releases() {
        let _ = backend.inject(event);
    }
    match result {
        ferrokey_net::Error::Io(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            println!("{device} disconnected");
        }
        err => println!("{device} disconnected: {err}"),
    }
}

/// Buttons and keys a client currently holds down.
#[derive(Default)]
struct Held {
    buttons: HashSet<MouseButton>,
    keys: HashSet<u32>,
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
            InputEvent::MouseMove { .. } | InputEvent::Scroll { .. } => {}
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
        buttons.chain(keys)
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
