//! The connecting side: pairs with or connects to a server and sends events.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use ferrokey_protocol::InputEvent;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::*;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// An authenticated connection to a server.
pub struct Session {
    channel: Channel,
    server_key: PublicKey,
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

    /// Sends `events`, split into as few messages as possible.
    pub fn send_events(&mut self, events: &[InputEvent]) -> Result<()> {
        let per_message = Channel::MAX_PAYLOAD_LEN / InputEvent::MAX_ENCODED_LEN;
        let mut payload = Vec::new();
        for chunk in events.chunks(per_message) {
            payload.clear();
            chunk.iter().for_each(|event| event.encode(&mut payload));
            self.channel.send(&payload)?;
        }
        Ok(())
    }

    /// Tells the server the client is still there; see [`KEEPALIVE_INTERVAL`].
    pub fn send_keepalive(&mut self) -> Result<()> {
        self.channel.send(&[])
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

    let mut channel = Channel {
        stream,
        noise: noise.into_transport_mode()?,
        buf: Vec::new(),
    };
    let verdict = match channel.recv() {
        Ok(verdict) => verdict,
        // A wrong code makes the server's last handshake step fail, and all
        // it can do then is hang up.
        Err(Error::Io(err)) if psk.is_some() && is_disconnect(&err) => {
            return Err(Error::PairingFailed);
        }
        Err(err) => return Err(err),
    };
    match verdict.as_slice() {
        [VERDICT_ACCEPTED] => {}
        [VERDICT_UNKNOWN_DEVICE] => return Err(Error::UnknownDevice),
        _ => return Err(Error::Protocol("unknown verdict")),
    }

    // The client never reads again, so only writes need a deadline.
    channel.stream.set_read_timeout(None)?;
    Ok(Session {
        channel,
        server_key,
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
