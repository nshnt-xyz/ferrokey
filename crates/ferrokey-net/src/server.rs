//! The accepting side: authenticates clients and receives their events.

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

/// An authenticated connection from a paired client.
pub struct Session {
    channel: Channel,
    device_key: PublicKey,
    device_name: String,
    newly_paired: bool,
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

    /// Waits for the next message from the client.
    ///
    /// Returns an empty batch for a keepalive, and fails if the client has
    /// been silent for longer than keepalives allow.
    pub fn recv_events(&mut self) -> Result<Vec<InputEvent>> {
        let payload = self.channel.recv()?;
        InputEvent::decode_all(&payload)
            .collect::<Result<_, _>>()
            .map_err(|_| Error::Protocol("malformed input event"))
    }

    pub fn shutdown(&self) {
        self.channel.shutdown();
    }
}

/// Runs the server side of the handshake on a freshly accepted connection.
pub fn accept(mut stream: TcpStream, keypair: &Keypair, auth: &impl Authorizer) -> Result<Session> {
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

    let mut channel = Channel {
        stream,
        noise: noise.into_transport_mode()?,
        buf: Vec::new(),
    };
    let newly_paired = psk.is_some();
    if newly_paired {
        auth.add_device(&device_key, &device_name)?;
    } else if !auth.is_trusted(&device_key) {
        channel.send(&[VERDICT_UNKNOWN_DEVICE])?;
        return Err(Error::UnknownDevice);
    }
    channel.send(&[VERDICT_ACCEPTED])?;

    channel.stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    Ok(Session {
        channel,
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
