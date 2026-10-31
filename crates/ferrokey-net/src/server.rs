//! The accepting side: authenticates clients, receives their events and
//! sends screen frames.

use std::io::{Read, Write};
use std::net::TcpStream;

use ferrokey_protocol::InputEvent;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::*;

/// The server's pairing and trust policy, consulted during [`accept`].
pub trait Authorizer {
    /// The code clients must enter to pair, or `None` if pairing is closed.
    fn pairing_code(&self) -> Option<String>;
    /// Called when a client entered the wrong pairing code.
    fn pairing_failed(&self);
    /// Remembers a newly paired device.
    fn add_device(&self, key: &PublicKey, name: &str) -> io::Result<()>;
    /// Whether `key` belongs to a device paired earlier.
    fn is_trusted(&self, key: &PublicKey) -> bool;
}

/// What the server offers clients, announced during [`accept`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    /// The server can share its screen.
    pub screen: bool,
}

/// An authenticated connection from a paired client.
///
/// [`Session::split`] separates it into a [`Sender`] and a [`Receiver`] for
/// use from different threads.
pub struct Session {
    sender: Sender,
    receiver: Receiver,
    device_key: PublicKey,
    device_name: String,
    newly_paired: bool,
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

/// A message from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    Keepalive,
    Input(Vec<InputEvent>),
    /// Start sending screen frames that fit in `max_width` by `max_height`,
    /// in either orientation.
    StartScreen {
        max_width: u16,
        max_height: u16,
    },
    StopScreen,
    /// The client is done with one frame; see [`FRAMES_IN_FLIGHT`].
    FrameAck,
}

impl Session {
    pub fn device_key(&self) -> PublicKey {
        self.device_key
    }

    /// The name the device reported for itself during the handshake.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Whether this connection paired the device.
    pub fn newly_paired(&self) -> bool {
        self.newly_paired
    }

    pub fn recv(&mut self) -> Result<ClientMessage> {
        self.receiver.recv()
    }

    pub fn shutdown(&self) {
        self.sender.shutdown();
    }

    pub fn split(self) -> (Sender, Receiver) {
        (self.sender, self.receiver)
    }
}

impl Receiver {
    /// Waits for the next message from the client, failing if it has been
    /// silent for longer than keepalives allow.
    pub fn recv(&mut self) -> Result<ClientMessage> {
        let payload = self.channel.recv()?;
        let Some((&kind, body)) = payload.split_first() else {
            return Ok(ClientMessage::Keepalive);
        };
        match (kind, body) {
            (MSG_INPUT, events) => InputEvent::decode_all(events)
                .collect::<Result<_, _>>()
                .map(ClientMessage::Input)
                .map_err(|_| Error::Protocol("malformed input event")),
            (MSG_START_SCREEN, &[w0, w1, h0, h1]) => Ok(ClientMessage::StartScreen {
                max_width: u16::from_be_bytes([w0, w1]),
                max_height: u16::from_be_bytes([h0, h1]),
            }),
            (MSG_STOP_SCREEN, []) => Ok(ClientMessage::StopScreen),
            (MSG_FRAME_ACK, []) => Ok(ClientMessage::FrameAck),
            _ => Err(Error::Protocol("unknown message from client")),
        }
    }

    pub fn shutdown(&self) {
        self.channel.shutdown();
    }
}

impl Sender {
    /// Sends a screen frame, a `width` by `height` JPEG image, split over as
    /// many messages as it takes.
    pub fn send_frame(&mut self, width: u16, height: u16, jpeg: &[u8]) -> Result<()> {
        if jpeg.len() > MAX_FRAME_LEN {
            return Err(Error::Protocol("frame too large"));
        }
        let (first, mut rest) =
            jpeg.split_at(jpeg.len().min(Channel::MAX_PAYLOAD_LEN - FRAME_HEADER_LEN));
        self.payload.clear();
        self.payload.push(MSG_FRAME);
        self.payload.extend_from_slice(&width.to_be_bytes());
        self.payload.extend_from_slice(&height.to_be_bytes());
        self.payload
            .extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
        self.payload.extend_from_slice(first);
        self.channel.send(&self.payload)?;
        while !rest.is_empty() {
            let (chunk, tail) = rest.split_at(rest.len().min(Channel::MAX_PAYLOAD_LEN - 1));
            rest = tail;
            self.payload.clear();
            self.payload.push(MSG_FRAME_DATA);
            self.payload.extend_from_slice(chunk);
            self.channel.send(&self.payload)?;
        }
        Ok(())
    }

    /// Tells the client the screen is not being shared (any more).
    pub fn send_screen_stopped(&mut self) -> Result<()> {
        self.channel.send(&[MSG_SCREEN_STOPPED])
    }

    /// Closes the connection, which also ends a blocked [`Receiver::recv`].
    pub fn shutdown(&self) {
        self.channel.shutdown();
    }
}

/// Runs the server side of the handshake on a freshly accepted connection.
pub fn accept(
    mut stream: TcpStream,
    keypair: &Keypair,
    auth: &impl Authorizer,
    features: Features,
) -> Result<Session> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let mut hello = [0; HELLO_LEN];
    stream.read_exact(&mut hello)?;
    if &hello[..4] != MAGIC {
        return Err(Error::Protocol("not a ferrokey client"));
    }
    let reply = |stream: &mut TcpStream, status: u8| stream.write_all(&[status]);
    if hello[4] != VERSION {
        reply(&mut stream, STATUS_UNSUPPORTED_VERSION)?;
        return Err(Error::UnsupportedVersion);
    }
    let code = match hello[5] {
        MODE_CONNECT => None,
        MODE_PAIR => match auth.pairing_code() {
            Some(code) => Some(code),
            None => {
                reply(&mut stream, STATUS_PAIRING_UNAVAILABLE)?;
                return Err(Error::PairingUnavailable);
            }
        },
        _ => return Err(Error::Protocol("unknown mode")),
    };
    reply(&mut stream, STATUS_OK)?;

    let mut prologue = [PROLOGUE_PREFIX, &hello, &[STATUS_OK]].concat();
    let psk = match &code {
        Some(code) => {
            let inbound = read_frame(&mut stream)?;
            let (spake, outbound) = Spake2::<Ed25519Group>::start_b(
                &Password::new(code),
                &Identity::new(SPAKE2_CLIENT_ID),
                &Identity::new(SPAKE2_SERVER_ID),
            );
            write_frame(&mut stream, &outbound)?;
            prologue.extend_from_slice(&inbound);
            prologue.extend_from_slice(&outbound);
            let key = spake
                .finish(&inbound)
                .map_err(|_| Error::Protocol("bad SPAKE2 message"))?;
            Some(spake2_key(key)?)
        }
        None => None,
    };

    let mut noise = noise_builder(keypair, &prologue, psk.as_ref())?.build_responder()?;
    recv_handshake(&mut stream, &mut noise)?;
    send_handshake(&mut stream, &mut noise, &[])?;
    let device_name = match recv_handshake(&mut stream, &mut noise) {
        Ok(payload) => sanitize_device_name(&payload),
        // In pairing mode the last message is the first one keyed by the
        // code, so failing to decrypt it means the client had the wrong one.
        Err(Error::Noise(snow::Error::Decrypt)) if psk.is_some() => {
            auth.pairing_failed();
            return Err(Error::PairingFailed);
        }
        Err(err) => return Err(err),
    };
    let device_key = PublicKey::from_slice(
        noise
            .get_remote_static()
            .ok_or(Error::Protocol("no client key"))?,
    )?;

    let mut channel = Channel::new(stream, noise)?;
    let newly_paired = psk.is_some();
    if newly_paired {
        auth.add_device(&device_key, &device_name)?;
    } else if !auth.is_trusted(&device_key) {
        channel.send(&[VERDICT_UNKNOWN_DEVICE])?;
        return Err(Error::UnknownDevice);
    }
    let feature_bits = if features.screen { FEATURE_SCREEN } else { 0 };
    channel.send(&[VERDICT_ACCEPTED, feature_bits])?;

    // Keepalives arrive well within the idle timeout. The handshake's write
    // timeout stays, so a client that stops reading can't stall the server.
    channel.stream().set_read_timeout(Some(IDLE_TIMEOUT))?;
    let (writer, reader) = channel.split();
    Ok(Session {
        sender: Sender {
            channel: writer,
            payload: Vec::new(),
        },
        receiver: Receiver { channel: reader },
        device_key,
        device_name,
        newly_paired,
    })
}

/// Keeps device names printable and short, since they end up in logs and
/// the server's device list.
fn sanitize_device_name(raw: &[u8]) -> String {
    let name: String = String::from_utf8_lossy(raw)
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME_LEN)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "unnamed device".to_owned()
    } else {
        name.to_owned()
    }
}
