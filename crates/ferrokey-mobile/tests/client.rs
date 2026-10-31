use std::collections::HashSet;
use std::io;
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;

use ferrokey_mobile::{Client, ClientError};
use ferrokey_net::server::{self, Authorizer};
use ferrokey_net::{Keypair, PublicKey};
use ferrokey_protocol::InputEvent;
use ferrokey_protocol::keys::{KEY_ENTER, KEY_LEFTCTRL, KEY_LEFTSHIFT, Modifiers, SpecialKey};

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
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            let mut session = server::accept(stream, &server_key, &auth).unwrap();
            // Events can arrive split over several messages; read until the
            // client disconnects.
            while let Ok(events) = session.recv_events() {
                received.extend(events);
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
    client.send(InputEvent::MouseMove { dx: 1, dy: 2 }).unwrap();
    client.disconnect();
    assert!(!client.is_connected());

    // A fresh Client reads the same key and paired server back from disk.
    let client = Client::new(data_dir.to_string_lossy().into(), "test phone".into()).unwrap();
    let servers = client.paired_servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].address, address);
    client.connect(address).unwrap();
    assert_eq!(
        client.type_text("A€".into(), Modifiers::default()).unwrap(),
        1
    );
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    client.tap_key(SpecialKey::Enter, ctrl).unwrap();
    client.disconnect();

    let key = |code, pressed| InputEvent::Key { code, pressed };
    assert_eq!(
        server.join().unwrap(),
        [
            InputEvent::MouseMove { dx: 1, dy: 2 },
            key(KEY_LEFTSHIFT, true),
            key(30, true),
            key(30, false),
            key(KEY_LEFTSHIFT, false),
            key(KEY_LEFTCTRL, true),
            key(KEY_ENTER, true),
            key(KEY_ENTER, false),
            key(KEY_LEFTCTRL, false),
        ]
    );
    std::fs::remove_dir_all(&data_dir).unwrap();
}
