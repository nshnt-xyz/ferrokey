//! Kotlin-facing API for the Android app, exported through UniFFI.
//!
//! The Kotlin bindings are generated from this crate at build time, so the
//! app always speaks the same [`InputEvent`] types as the server.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;

use ferrokey_net::{KEEPALIVE_INTERVAL, Keypair, PublicKey, client};
use ferrokey_protocol::keys::{self, Modifiers, SpecialKey};
use ferrokey_protocol::{InputEvent, MouseButton};

uniffi::setup_scaffolding!();

#[uniffi::remote(Enum)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: MouseButton, pressed: bool },
    Scroll { dx: i32, dy: i32 },
    Key { code: u32, pressed: bool },
}

#[uniffi::remote(Enum)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[uniffi::remote(Enum)]
pub enum SpecialKey {
    Escape,
    Tab,
    Backspace,
    Enter,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[uniffi::remote(Record)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum ClientError {
    #[error("{0}")]
    Connection(#[from] ferrokey_net::Error),
    #[error("not paired with {0}")]
    NotPaired(String),
    #[error("not connected")]
    NotConnected,
    #[error("storage error: {0}")]
    Storage(#[from] io::Error),
}

/// A server this device has paired with.
#[derive(uniffi::Record)]
pub struct PairedServer {
    pub address: String,
    /// Matches the fingerprint the server prints on startup.
    pub fingerprint: String,
}

/// This device's identity, its paired servers, and the current connection.
///
/// `pair` and `connect` block on the network, so call them off the main
/// thread. `send` only queues the event and is safe to call from anywhere.
#[derive(uniffi::Object)]
pub struct Client {
    data_dir: PathBuf,
    device_name: String,
    keypair: Keypair,
    servers: Mutex<Vec<(String, PublicKey)>>,
    connection: Mutex<Option<Connection>>,
}

#[uniffi::export]
impl Client {
    /// Loads or creates this device's key and paired servers in `data_dir`.
    #[uniffi::constructor]
    pub fn new(data_dir: String, device_name: String) -> Result<Arc<Self>, ClientError> {
        let data_dir = PathBuf::from(data_dir);
        fs::create_dir_all(&data_dir)?;
        Ok(Arc::new(Self {
            keypair: load_or_create_keypair(&data_dir.join("device.key"))?,
            servers: Mutex::new(load_servers(&data_dir.join("servers"))?),
            data_dir,
            device_name,
            connection: Mutex::new(None),
        }))
    }

    pub fn paired_servers(&self) -> Vec<PairedServer> {
        self.servers
            .lock()
            .unwrap()
            .iter()
            .map(|(address, key)| PairedServer {
                address: address.clone(),
                fingerprint: key.fingerprint(),
            })
            .collect()
    }

    /// Pairs with the server at `address` using the code it displays, then
    /// stays connected to it.
    pub fn pair(&self, address: String, code: String) -> Result<(), ClientError> {
        self.disconnect();
        let address = address.trim().to_owned();
        let session = client::pair(&address, &code, &self.keypair, &self.device_name)?;

        let mut servers = self.servers.lock().unwrap();
        servers.retain(|(existing, _)| *existing != address);
        servers.push((address, session.server_key()));
        save_servers(&self.data_dir.join("servers"), &servers)?;
        drop(servers);

        *self.connection.lock().unwrap() = Some(Connection::start(session));
        Ok(())
    }

    /// Connects to a server paired earlier.
    pub fn connect(&self, address: String) -> Result<(), ClientError> {
        self.disconnect();
        let server_key = self
            .servers
            .lock()
            .unwrap()
            .iter()
            .find(|(existing, _)| *existing == address)
            .map(|(_, key)| *key)
            .ok_or_else(|| ClientError::NotPaired(address.clone()))?;
        let session = client::connect(&address, &self.keypair, &server_key, &self.device_name)?;
        *self.connection.lock().unwrap() = Some(Connection::start(session));
        Ok(())
    }

    /// Removes a paired server. Pairing with it again needs a new code.
    pub fn forget(&self, address: String) -> Result<(), ClientError> {
        let mut servers = self.servers.lock().unwrap();
        servers.retain(|(existing, _)| *existing != address);
        save_servers(&self.data_dir.join("servers"), &servers)?;
        Ok(())
    }

    pub fn disconnect(&self) {
        // Dropping the connection's sender stops its thread.
        self.connection.lock().unwrap().take();
    }

    /// Whether a connection is open. Turns false shortly after the server
    /// goes away, once a send or keepalive fails.
    pub fn is_connected(&self) -> bool {
        self.connection
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(Connection::is_alive)
    }

    pub fn send(&self, event: InputEvent) -> Result<(), ClientError> {
        self.send_all([event])
    }

    /// Types `text` as key presses, assuming the computer uses a US QWERTY
    /// layout, while holding `modifiers`. Returns how many characters were
    /// skipped because no key on that layout types them.
    pub fn type_text(&self, text: String, modifiers: Modifiers) -> Result<u32, ClientError> {
        let mut events = Vec::new();
        let skipped = keys::type_text(&text, modifiers, &mut events);
        self.send_all(events)?;
        Ok(skipped.try_into().unwrap_or(u32::MAX))
    }

    /// Presses and releases `key` while holding `modifiers`.
    pub fn tap_key(&self, key: SpecialKey, modifiers: Modifiers) -> Result<(), ClientError> {
        let mut events = Vec::new();
        keys::tap(key.code(), modifiers, &mut events);
        self.send_all(events)
    }
}

impl Client {
    fn send_all(&self, events: impl IntoIterator<Item = InputEvent>) -> Result<(), ClientError> {
        let connection = self.connection.lock().unwrap();
        let connection = connection
            .as_ref()
            .filter(|c| c.is_alive())
            .ok_or(ClientError::NotConnected)?;
        for event in events {
            connection
                .events
                .send(event)
                .map_err(|_| ClientError::NotConnected)?;
        }
        Ok(())
    }
}

/// An open session, written to by a dedicated thread so that sending never
/// blocks the caller.
struct Connection {
    events: mpsc::Sender<InputEvent>,
    alive: Arc<AtomicBool>,
}

impl Connection {
    fn start(mut session: client::Session) -> Self {
        let (events, rx) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = alive.clone();
        thread::spawn(move || {
            let mut batch = Vec::new();
            loop {
                let result = match rx.recv_timeout(KEEPALIVE_INTERVAL) {
                    Ok(event) => {
                        // Coalesce whatever queued up while the last write ran.
                        batch.clear();
                        batch.push(event);
                        batch.extend(rx.try_iter());
                        session.send_events(&batch)
                    }
                    Err(RecvTimeoutError::Timeout) => session.send_keepalive(),
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if result.is_err() {
                    break;
                }
            }
            thread_alive.store(false, Ordering::Relaxed);
            session.shutdown();
        });
        Self { events, alive }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

fn load_or_create_keypair(path: &Path) -> io::Result<Keypair> {
    match fs::read(path) {
        Ok(bytes) => Keypair::from_bytes(&bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "device key is corrupt")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let keypair = Keypair::generate().map_err(io::Error::other)?;
            fs::write(path, keypair.to_bytes())?;
            Ok(keypair)
        }
        Err(err) => Err(err),
    }
}

/// Reads `<hex server key> <address>` lines.
fn load_servers(path: &Path) -> io::Result<Vec<(String, PublicKey)>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(contents
        .lines()
        .filter_map(|line| {
            let (key, address) = line.split_once(' ')?;
            Some((address.to_owned(), PublicKey::from_hex(key)?))
        })
        .collect())
}

fn save_servers(path: &Path, servers: &[(String, PublicKey)]) -> io::Result<()> {
    let contents: String = servers
        .iter()
        .map(|(address, key)| format!("{} {address}\n", key.to_hex()))
        .collect();
    fs::write(path, contents)
}
