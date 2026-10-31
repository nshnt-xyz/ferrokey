//! The connecting side: pairs with or connects to a server, sends events and
//! receives screen frames.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use ferrokey_protocol::InputEvent;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::*;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// An authenticated connection to a server.
///
/// [`Session::split`] separates it into a [`Sender`] and a [`Receiver`] for
/// use from different threads.
pub struct Session {
    sender: Sender,
    receiver: Receiver,
    server_key: PublicKey,
    features: u8,
}

/// The sending half of a [`Session`].
pub struct Sender {
    channel: ChannelWriter,
    payload: Vec<u8>,
}

/// The receiving half of a [`Session`].
pub struct Receiver {
    channel: ChannelReader,
}

/// A message from the server.
#[derive(Debug, PartialEq, Eq)]
pub enum ServerMessage {
    /// A screen frame. Acknowledge it with [`Sender::ack_frame`] once it has
    /// been shown, or the server stops sending more.
    Frame(Frame),
    /// The server is not sharing its screen (any more).
    ScreenStopped,
}

/// One screen frame, a JPEG image.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub jpeg: Vec<u8>,
}

/// Pairs with the server at `address` using the code it is displaying.
///
/// On success the returned session is connected, and
/// [`Session::server_key`] is the key to pin for later [`connect`] calls.
pub fn pair(address: &str, code: &str, keypair: &Keypair, device_name: &str) -> Result<Session> {
    let code: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    handshake(address, keypair, device_name, Pairing::New { code: &code })
}

/// Connects to a server this device has already paired with.
pub fn connect(
    address: &str,
    keypair: &Keypair,
    server_key: &PublicKey,
    device_name: &str,
) -> Result<Session> {
    handshake(
        address,
        keypair,
        device_name,
        Pairing::Existing { server_key },
    )
}

impl Session {
    pub fn server_key(&self) -> PublicKey {
        self.server_key
    }

    /// Whether the server can share its screen; see [`Sender::start_screen`].
    pub fn screen_available(&self) -> bool {
        self.features & FEATURE_SCREEN != 0
    }

    pub fn send_events(&mut self, events: &[InputEvent]) -> Result<()> {
        self.sender.send_events(events)
    }

    pub fn send_keepalive(&mut self) -> Result<()> {
        self.sender.send_keepalive()
    }

    pub fn recv(&mut self) -> Result<ServerMessage> {
        self.receiver.recv()
    }

    pub fn shutdown(&self) {
        self.sender.shutdown();
    }

    pub fn split(self) -> (Sender, Receiver) {
        (self.sender, self.receiver)
    }
}

impl Sender {
    /// Sends `events`, split into as few messages as possible.
    pub fn send_events(&mut self, events: &[InputEvent]) -> Result<()> {
        let per_message = (Channel::MAX_PAYLOAD_LEN - 1) / InputEvent::MAX_ENCODED_LEN;
        for chunk in events.chunks(per_message) {
            self.payload.clear();
            self.payload.push(MSG_INPUT);
            chunk
                .iter()
                .for_each(|event| event.encode(&mut self.payload));
            self.channel.send(&self.payload)?;
        }
        Ok(())
    }

    /// Tells the server the client is still there; see [`KEEPALIVE_INTERVAL`].
    pub fn send_keepalive(&mut self) -> Result<()> {
        self.channel.send(&[])
    }

    /// Asks the server to start sending screen frames no larger than
    /// `max_width` by `max_height`, in either orientation.
    pub fn start_screen(&mut self, max_width: u16, max_height: u16) -> Result<()> {
        let mut message = [MSG_START_SCREEN, 0, 0, 0, 0];
        message[1..3].copy_from_slice(&max_width.to_be_bytes());
        message[3..5].copy_from_slice(&max_height.to_be_bytes());
        self.channel.send(&message)
    }

    pub fn stop_screen(&mut self) -> Result<()> {
        self.channel.send(&[MSG_STOP_SCREEN])
    }

    /// Acknowledges a [`ServerMessage::Frame`], letting the server send
    /// another.
    pub fn ack_frame(&mut self) -> Result<()> {
        self.channel.send(&[MSG_FRAME_ACK])
    }

    /// Closes the connection, which also ends a blocked [`Receiver::recv`].
    pub fn shutdown(&self) {
        self.channel.shutdown();
    }
}

impl Receiver {
    /// Waits for the next message from the server.
    pub fn recv(&mut self) -> Result<ServerMessage> {
        let message = self.channel.recv()?;
        match message.split_first() {
            Some((&MSG_FRAME, rest)) => self.recv_frame(rest).map(ServerMessage::Frame),
            Some((&MSG_SCREEN_STOPPED, [])) => Ok(ServerMessage::ScreenStopped),
            _ => Err(Error::Protocol("unexpected message from server")),
        }
    }

    /// Reassembles a frame from its first message, whose kind byte is
    /// already gone, and however many data messages follow.
    fn recv_frame(&mut self, header: &[u8]) -> Result<Frame> {
        let bad = || Error::Protocol("malformed frame");
        let (size, data) = header.split_first_chunk::<8>().ok_or_else(bad)?;
        let width = u16::from_be_bytes([size[0], size[1]]);
        let height = u16::from_be_bytes([size[2], size[3]]);
        let len = u32::from_be_bytes([size[4], size[5], size[6], size[7]]) as usize;
        if len > MAX_FRAME_LEN || data.len() > len {
            return Err(bad());
        }
        let mut jpeg = Vec::with_capacity(len);
        jpeg.extend_from_slice(data);
        while jpeg.len() < len {
            let message = self.channel.recv()?;
            match message.split_first() {
                Some((&MSG_FRAME_DATA, data)) if jpeg.len() + data.len() <= len => {
                    jpeg.extend_from_slice(data);
                }
                _ => return Err(bad()),
            }
        }
        Ok(Frame {
            width,
            height,
            jpeg,
        })
    }

    pub fn shutdown(&self) {
        self.channel.shutdown();
    }
}

enum Pairing<'a> {
    New { code: &'a str },
    Existing { server_key: &'a PublicKey },
}

fn handshake(
    address: &str,
    keypair: &Keypair,
    device_name: &str,
    pairing: Pairing,
) -> Result<Session> {
    let mut stream = open(address)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let mode = match pairing {
        Pairing::New { .. } => MODE_PAIR,
        Pairing::Existing { .. } => MODE_CONNECT,
    };
    let mut hello = [0; HELLO_LEN];
    hello[..4].copy_from_slice(MAGIC);
    hello[4] = VERSION;
    hello[5] = mode;
    stream.write_all(&hello)?;

    let mut status = [0; 1];
    stream.read_exact(&mut status)?;
    match status[0] {
        STATUS_OK => {}
        STATUS_UNSUPPORTED_VERSION => return Err(Error::UnsupportedVersion),
        STATUS_PAIRING_UNAVAILABLE => return Err(Error::PairingUnavailable),
        _ => return Err(Error::Protocol("unknown status")),
    }

    let mut prologue = [PROLOGUE_PREFIX, &hello, &status].concat();
    let psk = match pairing {
        Pairing::New { code } => {
            let (spake, outbound) = Spake2::<Ed25519Group>::start_a(
                &Password::new(code),
                &Identity::new(SPAKE2_CLIENT_ID),
                &Identity::new(SPAKE2_SERVER_ID),
            );
            write_frame(&mut stream, &outbound)?;
            let inbound = read_frame(&mut stream)?;
            prologue.extend_from_slice(&outbound);
            prologue.extend_from_slice(&inbound);
            let key = spake
                .finish(&inbound)
                .map_err(|_| Error::Protocol("bad SPAKE2 message"))?;
            Some(spake2_key(key)?)
        }
        Pairing::Existing { .. } => None,
    };

    let mut noise = noise_builder(keypair, &prologue, psk.as_ref())?.build_initiator()?;
    send_handshake(&mut stream, &mut noise, &[])?;
    recv_handshake(&mut stream, &mut noise)?;

    let server_key = PublicKey::from_slice(
        noise
            .get_remote_static()
            .ok_or(Error::Protocol("no server key"))?,
    )?;
    if let Pairing::Existing { server_key: pinned } = pairing
        && server_key != *pinned
    {
        // Stop before the next message, which would reveal our identity.
        return Err(Error::ServerKeyMismatch);
    }
    send_handshake(&mut stream, &mut noise, device_name.as_bytes())?;

    let mut channel = Channel::new(stream, noise)?;
    let verdict = match channel.recv() {
        Ok(verdict) => verdict,
        // A wrong code makes the server's last handshake step fail, and all
        // it can do then is hang up.
        Err(Error::Io(err)) if psk.is_some() && is_disconnect(&err) => {
            return Err(Error::PairingFailed);
        }
        Err(err) => return Err(err),
    };
    let features = match verdict.as_slice() {
        [VERDICT_ACCEPTED, features] => *features,
        [VERDICT_UNKNOWN_DEVICE] => return Err(Error::UnknownDevice),
        _ => return Err(Error::Protocol("unknown verdict")),
    };

    // The server only speaks when asked to, so waiting for it is open-ended;
    // the client notices a dead server when a write fails.
    channel.stream().set_read_timeout(None)?;
    let (writer, reader) = channel.split();
    Ok(Session {
        sender: Sender {
            channel: writer,
            payload: Vec::new(),
        },
        receiver: Receiver { channel: reader },
        server_key,
        features,
    })
}

fn is_disconnect(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    )
}

/// Connects to `address`, which is `host` or `host:port`; the port defaults
/// to [`DEFAULT_PORT`].
fn open(address: &str) -> Result<TcpStream> {
    let address = address.trim();
    let candidates: Vec<SocketAddr> = if let Ok(ip) = address.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, DEFAULT_PORT)]
    } else if let Ok(addrs) = address.to_socket_addrs() {
        addrs.collect()
    } else {
        (address, DEFAULT_PORT)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default()
    };

    let mut last_err = None;
    for candidate in candidates {
        match TcpStream::connect_timeout(&candidate, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.map_or_else(|| Error::BadAddress(address.to_owned()), Error::Io))
}
