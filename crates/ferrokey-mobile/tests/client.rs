use std::collections::HashSet;
use std::io;
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ferrokey_mobile::{Client, ClientError, ScreenFrame, ScreenListener};
use ferrokey_net::server::{self, Authorizer, ClientMessage, Features};
use ferrokey_net::{Keypair, PublicKey};
use ferrokey_protocol::InputEvent;
use ferrokey_protocol::keys::{KEY_ENTER, KEY_LEFTCTRL, Modifiers, SpecialKey};

struct Auth {
    code: String,
    devices: Mutex<HashSet<PublicKey>>,
}

impl Authorizer for Auth {
    fn pairing_code(&self) -> Option<String> {
        Some(self.code.clone())
    }

    fn pairing_failed(&self) {}

    fn add_device(&self, key: &PublicKey, _name: &str) -> io::Result<()> {
        self.devices.lock().unwrap().insert(*key);
        Ok(())
    }

    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.devices.lock().unwrap().contains(key)
    }
}

/// What a [`ScreenListener`] was told, in order.
#[derive(Debug, PartialEq)]
enum Seen {
    Frame(u32, u32, Vec<u8>),
    Stopped,
}

struct Listener(Mutex<mpsc::Sender<Seen>>);

impl ScreenListener for Listener {
    fn on_frame(&self, frame: ScreenFrame) {
        let seen = Seen::Frame(frame.width, frame.height, frame.jpeg);
        self.0.lock().unwrap().send(seen).unwrap();
    }

    fn on_stopped(&self) {
        self.0.lock().unwrap().send(Seen::Stopped).unwrap();
    }
}

#[test]
fn pairs_persists_and_reconnects() {
    let data_dir =
        std::env::temp_dir().join(format!("ferrokey-mobile-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server_key = Keypair::generate().unwrap();
    let auth = Auth {
        code: "246810".into(),
        devices: Mutex::default(),
    };
    let server = thread::spawn(move || {
        let mut received = Vec::new();
        for screen in [false, true] {
            let (stream, _) = listener.accept().unwrap();
            let session = server::accept(stream, &server_key, &auth, Features { screen }).unwrap();
            let (mut sender, mut receiver) = session.split();
            // Events can arrive split over several messages; read until the
            // client disconnects. Answer a screen request with one frame,
            // and its acknowledgement by stopping.
            while let Ok(message) = receiver.recv() {
                match message {
                    ClientMessage::Input(events) => received.extend(events),
                    ClientMessage::StartScreen {
                        max_width,
                        max_height,
                    } => {
                        assert_eq!((max_width, max_height), (800, u16::MAX));
                        sender.send_frame(4, 3, b"jpeg").unwrap();
                    }
                    ClientMessage::FrameAck => sender.send_screen_stopped().unwrap(),
                    ClientMessage::StopScreen | ClientMessage::Keepalive => {}
                }
            }
        }
        received
    });

    let client = Client::new(data_dir.to_string_lossy().into(), "test phone".into()).unwrap();
    assert!(matches!(
        client.send(InputEvent::Scroll { dx: 0, dy: 1 }),
        Err(ClientError::NotConnected)
    ));
    client.pair(address.clone(), "246 810".into()).unwrap();
    assert!(client.is_connected());
    assert!(!client.screen_available());
    client.send(InputEvent::MouseMove { dx: 1, dy: 2 }).unwrap();
    client.disconnect();
    assert!(!client.is_connected());

    // A fresh Client reads the same key and paired server back from disk.
    let client = Client::new(data_dir.to_string_lossy().into(), "test phone".into()).unwrap();
    let servers = client.paired_servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].address, address);
    client.connect(address).unwrap();
    assert!(client.screen_available());
    let (seen_tx, seen) = mpsc::channel();
    client
        .start_screen(800, 100_000, Arc::new(Listener(Mutex::new(seen_tx))))
        .unwrap();
    let timeout = Duration::from_secs(5);
    assert_eq!(
        seen.recv_timeout(timeout).unwrap(),
        Seen::Frame(4, 3, b"jpeg".to_vec())
    );
    assert_eq!(seen.recv_timeout(timeout).unwrap(), Seen::Stopped);
    client.point_at(0.5, 2.0).unwrap();
    assert_eq!(
        client
            .type_text("A€\u{7}".into(), Modifiers::default())
            .unwrap(),
        1
    );
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    client.tap_key(SpecialKey::Enter, ctrl).unwrap();
    client.disconnect();

    let key = |code, pressed| InputEvent::Key { code, pressed };
    let keysym = |keysym, pressed| InputEvent::Keysym { keysym, pressed };
    assert_eq!(
        server.join().unwrap(),
        [
            InputEvent::MouseMove { dx: 1, dy: 2 },
            InputEvent::MouseMoveAbsolute { x: 32768, y: 65535 },
            keysym(0x41, true),
            keysym(0x41, false),
            keysym(0x0100_20ac, true),
            keysym(0x0100_20ac, false),
            key(KEY_LEFTCTRL, true),
            key(KEY_ENTER, true),
            key(KEY_ENTER, false),
            key(KEY_LEFTCTRL, false),
        ]
    );
    std::fs::remove_dir_all(&data_dir).unwrap();
}
