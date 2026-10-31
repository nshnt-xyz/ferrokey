use std::collections::HashSet;
use std::io;
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;

use ferrokey_net::client::{Frame, ServerMessage};
use ferrokey_net::server::{Authorizer, ClientMessage, Features};
use ferrokey_net::{Error, Keypair, PublicKey, client, server};
use ferrokey_protocol::{InputEvent, MouseButton};

#[derive(Default)]
struct TestAuth {
    code: Mutex<Option<String>>,
    failures: Mutex<u32>,
    devices: Mutex<HashSet<PublicKey>>,
}

impl TestAuth {
    fn pairing(code: &str) -> Self {
        Self {
            code: Mutex::new(Some(code.to_owned())),
            ..Default::default()
        }
    }
}

impl Authorizer for TestAuth {
    fn pairing_code(&self) -> Option<String> {
        self.code.lock().unwrap().clone()
    }

    fn pairing_failed(&self) {
        *self.failures.lock().unwrap() += 1;
    }

    fn add_device(&self, key: &PublicKey, _name: &str) -> io::Result<()> {
        self.devices.lock().unwrap().insert(*key);
        *self.code.lock().unwrap() = None;
        Ok(())
    }

    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.devices.lock().unwrap().contains(key)
    }
}

/// Accepts one connection on a background thread and runs `serve` on the
/// result of the server handshake.
fn serve_once<T: Send + 'static>(
    server_key: &Keypair,
    auth: &'static TestAuth,
    features: Features,
    serve: impl FnOnce(Result<server::Session, Error>) -> T + Send + 'static,
) -> (String, thread::JoinHandle<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server_key = server_key.clone();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(server::accept(stream, &server_key, auth, features))
    });
    (address, handle)
}

fn leak(auth: TestAuth) -> &'static TestAuth {
    Box::leak(Box::new(auth))
}

#[test]
fn pairs_then_reconnects_and_sends_events() {
    let server_key = Keypair::generate().unwrap();
    let device_key = Keypair::generate().unwrap();
    let auth = leak(TestAuth::pairing("123456"));

    let (address, server) = serve_once(&server_key, auth, Features::default(), |session| {
        let session = session.unwrap();
        (session.device_name().to_owned(), session.newly_paired())
    });
    let session = client::pair(&address, "123 456", &device_key, "Pixel\n9").unwrap();
    assert_eq!(session.server_key(), server_key.public());
    assert!(!session.screen_available());
    assert_eq!(server.join().unwrap(), ("Pixel9".to_owned(), true));
    assert!(auth.is_trusted(&device_key.public()));

    let events = [
        InputEvent::MouseMove { dx: 3, dy: -4 },
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    ];
    let (address, server) = serve_once(&server_key, auth, Features::default(), |session| {
        let mut session = session.unwrap();
        assert!(!session.newly_paired());
        let keepalive = session.recv().unwrap();
        (keepalive, session.recv().unwrap())
    });
    let mut session =
        client::connect(&address, &device_key, &server_key.public(), "Pixel 9").unwrap();
    session.send_keepalive().unwrap();
    session.send_events(&events).unwrap();
    assert_eq!(
        server.join().unwrap(),
        (
            ClientMessage::Keepalive,
            ClientMessage::Input(events.to_vec())
        )
    );
}

#[test]
fn streams_screen_frames_both_ways_at_once() {
    let server_key = Keypair::generate().unwrap();
    let device_key = Keypair::generate().unwrap();
    let auth = leak(TestAuth::default());
    auth.devices.lock().unwrap().insert(device_key.public());

    // Big enough to span several messages, and not a multiple of their size.
    let jpeg: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let server_jpeg = jpeg.clone();
    let (address, server) = serve_once(
        &server_key,
        auth,
        Features { screen: true },
        move |session| {
            let (mut sender, mut receiver) = session.unwrap().split();
            let start = receiver.recv().unwrap();
            // Send while the client is also sending, from another thread.
            let writer = thread::spawn(move || {
                sender.send_frame(640, 360, &server_jpeg).unwrap();
                sender.send_frame(2, 1, &[]).unwrap();
                sender.send_screen_stopped().unwrap();
                sender
            });
            let mut received = vec![start];
            while let Ok(message) = receiver.recv() {
                received.push(message);
            }
            drop(writer.join().unwrap());
            received
        },
    );

    let session = client::connect(&address, &device_key, &server_key.public(), "phone").unwrap();
    assert!(session.screen_available());
    let (mut sender, mut receiver) = session.split();
    sender.start_screen(1920, 1080).unwrap();
    let mut messages = Vec::new();
    for _ in 0..3 {
        let message = receiver.recv().unwrap();
        if matches!(message, ServerMessage::Frame(_)) {
            sender.ack_frame().unwrap();
        }
        messages.push(message);
    }
    sender
        .send_events(&[InputEvent::MouseMoveAbsolute { x: 1, y: 2 }])
        .unwrap();
    sender.stop_screen().unwrap();
    sender.shutdown();

    assert_eq!(
        messages,
        [
            ServerMessage::Frame(Frame {
                width: 640,
                height: 360,
                jpeg,
            }),
            ServerMessage::Frame(Frame {
                width: 2,
                height: 1,
                jpeg: vec![],
            }),
            ServerMessage::ScreenStopped,
        ]
    );
    assert_eq!(
        server.join().unwrap(),
        [
            ClientMessage::StartScreen {
                max_width: 1920,
                max_height: 1080,
            },
            ClientMessage::FrameAck,
            ClientMessage::FrameAck,
            ClientMessage::Input(vec![InputEvent::MouseMoveAbsolute { x: 1, y: 2 }]),
            ClientMessage::StopScreen,
        ]
    );
}

#[test]
fn wrong_code_fails_on_both_sides() {
    let server_key = Keypair::generate().unwrap();
    let auth = leak(TestAuth::pairing("123456"));

    let (address, server) = serve_once(&server_key, auth, Features::default(), |session| {
        session.err()
    });
    let result = client::pair(&address, "654321", &Keypair::generate().unwrap(), "phone");
    assert!(
        matches!(result, Err(Error::PairingFailed)),
        "{:?}",
        result.err()
    );
    assert!(matches!(server.join().unwrap(), Some(Error::PairingFailed)));
    assert_eq!(*auth.failures.lock().unwrap(), 1);
    assert!(auth.devices.lock().unwrap().is_empty());
}

#[test]
fn pairing_closed_is_reported() {
    let server_key = Keypair::generate().unwrap();
    let auth = leak(TestAuth::default());

    let (address, server) = serve_once(&server_key, auth, Features::default(), |session| {
        session.err()
    });
    let result = client::pair(&address, "123456", &Keypair::generate().unwrap(), "phone");
    assert!(
        matches!(result, Err(Error::PairingUnavailable)),
        "{:?}",
        result.err()
    );
    assert!(matches!(
        server.join().unwrap(),
        Some(Error::PairingUnavailable)
    ));
}

#[test]
fn unpaired_device_is_rejected() {
    let server_key = Keypair::generate().unwrap();
    let auth = leak(TestAuth::default());

    let (address, server) = serve_once(&server_key, auth, Features::default(), |session| {
        session.err()
    });
    let result = client::connect(
        &address,
        &Keypair::generate().unwrap(),
        &server_key.public(),
        "phone",
    );
    assert!(
        matches!(result, Err(Error::UnknownDevice)),
        "{:?}",
        result.err()
    );
    assert!(matches!(server.join().unwrap(), Some(Error::UnknownDevice)));
}

#[test]
fn impostor_server_is_rejected() {
    let real_server = Keypair::generate().unwrap();
    let impostor = Keypair::generate().unwrap();
    let auth = leak(TestAuth::default());

    let (address, server) = serve_once(&impostor, auth, Features::default(), |session| {
        session.err()
    });
    let result = client::connect(
        &address,
        &Keypair::generate().unwrap(),
        &real_server.public(),
        "phone",
    );
    assert!(
        matches!(result, Err(Error::ServerKeyMismatch)),
        "{:?}",
        result.err()
    );
    // The client hangs up before identifying itself.
    assert!(matches!(server.join().unwrap(), Some(Error::Io(_))));
}

#[test]
fn keys_round_trip() {
    let keypair = Keypair::generate().unwrap();
    let restored = Keypair::from_bytes(&keypair.to_bytes()).unwrap();
    assert_eq!(restored.public(), keypair.public());
    let public = keypair.public();
    assert_eq!(PublicKey::from_hex(&public.to_hex()), Some(public));
    assert_eq!(PublicKey::from_hex("zz"), None);
}
