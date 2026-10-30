//! The ferrokey client/server link: pairing, encryption and framing.
//!
//! Connections are TCP. Every message after a short plaintext preamble is
//! encrypted and authenticated with the [Noise protocol](https://noiseprotocol.org),
//! using an `XX` handshake in which both sides prove ownership of a
//! long-term X25519 key.
//!
//! **Pairing** introduces a new client. The server shows a short numeric code
//! and the user types it on the client. Both sides run SPAKE2 with that code
//! and feed the resulting key into the Noise handshake as a pre-shared key
//! (`XXpsk3`), so the handshake only completes if the codes matched. An
//! eavesdropper learns nothing that would let them test guesses offline, and
//! an active attacker gets one guess per attempt. Afterwards the server
//! remembers the client's key and the client pins the server's.
//!
//! **Connecting** uses plain `XX`: the client rejects any server whose key
//! differs from the pinned one, and the server rejects any client whose key
//! it has not paired with.
//!
//! Wire format, after the TCP connection opens:
//!
//! 1. Client sends `"FKEY" version mode` ([`HELLO_LEN`] bytes, unframed).
//! 2. Server answers with one status byte.
//! 3. Pairing only: client and server exchange framed SPAKE2 messages.
//! 4. The three Noise handshake messages, framed. The client's device name
//!    rides in the encrypted payload of the last one.
//! 5. Server sends an encrypted verdict: accepted, or unknown device.
//! 6. Client sends encrypted batches of [`InputEvent`]s; an empty message is
//!    a keepalive.
//!
//! Frames are a big-endian `u16` length followed by that many bytes. Every
//! plaintext byte from steps 1 to 3 is bound into the Noise prologue, so
//! tampering with them makes the handshake fail.
//!
//! [`InputEvent`]: ferrokey_protocol::InputEvent

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use snow::{HandshakeState, TransportState};

pub mod client;
pub mod server;

pub use ferrokey_protocol::DEFAULT_PORT;

const MAGIC: &[u8; 4] = b"FKEY";
const VERSION: u8 = 1;
const HELLO_LEN: usize = 6;

const MODE_CONNECT: u8 = 0;
const MODE_PAIR: u8 = 1;

const STATUS_OK: u8 = 0;
const STATUS_UNSUPPORTED_VERSION: u8 = 1;
const STATUS_PAIRING_UNAVAILABLE: u8 = 2;

const VERDICT_ACCEPTED: u8 = 0;
const VERDICT_UNKNOWN_DEVICE: u8 = 1;

const NOISE_CONNECT: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const NOISE_PAIR: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE_PREFIX: &[u8] = b"ferrokey v1";

const SPAKE2_CLIENT_ID: &[u8] = b"ferrokey client";
const SPAKE2_SERVER_ID: &[u8] = b"ferrokey server";

/// Largest Noise message, and therefore largest frame.
const MAX_MESSAGE_LEN: usize = 65535;
/// Noise's authentication tag on every transport message.
const TAG_LEN: usize = 16;

/// Device names longer than this are truncated by the server.
pub const MAX_DEVICE_NAME_LEN: usize = 64;

/// How long either side waits for the other during the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often an idle client sends a keepalive.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// The server drops a client it has not heard from for this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("encryption error: {0}")]
    Noise(#[from] snow::Error),
    #[error("protocol error: {0}")]
    Protocol(&'static str),
    #[error("the server speaks a different protocol version")]
    UnsupportedVersion,
    #[error("the server is not accepting new devices; restart it with --pair")]
    PairingUnavailable,
    #[error("pairing failed; check the code and try again")]
    PairingFailed,
    #[error("this device is not paired with the server")]
    UnknownDevice,
    #[error("the server's identity changed since pairing; pair again if this is expected")]
    ServerKeyMismatch,
    #[error("no address found for {0}")]
    BadAddress(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An X25519 public key identifying a server or a client device.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut key = [0; 32];
        for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(key))
    }

    /// A short form for showing to people, e.g. `3f2a 91c0 7b44 e1d8`.
    pub fn fingerprint(&self) -> String {
        self.0[..8]
            .chunks(2)
            .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn from_slice(bytes: &[u8]) -> Result<Self> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| Error::Protocol("bad public key"))
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({})", self.fingerprint())
    }
}

/// A long-term X25519 keypair.
#[derive(Clone)]
pub struct Keypair {
    private: [u8; 32],
    public: PublicKey,
}

impl Keypair {
    /// Length of [`Keypair::to_bytes`].
    pub const ENCODED_LEN: usize = 64;

    pub fn generate() -> Result<Self> {
        let keypair = snow::Builder::new(NOISE_CONNECT.parse()?).generate_keypair()?;
        Ok(Self {
            private: keypair
                .private
                .try_into()
                .map_err(|_| Error::Protocol("bad private key"))?,
            public: PublicKey::from_slice(&keypair.public)?,
        })
    }

    pub fn public(&self) -> PublicKey {
        self.public
    }

    /// Serializes the keypair for storage. Keep the result secret.
    pub fn to_bytes(&self) -> [u8; Self::ENCODED_LEN] {
        let mut bytes = [0; Self::ENCODED_LEN];
        bytes[..32].copy_from_slice(&self.private);
        bytes[32..].copy_from_slice(&self.public.0);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; Self::ENCODED_LEN] = bytes.try_into().ok()?;
        let (private, public) = bytes.split_at(32);
        Some(Self {
            private: private.try_into().ok()?,
            public: PublicKey(public.try_into().ok()?),
        })
    }
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// An encrypted, authenticated message stream over TCP.
pub struct Channel {
    stream: TcpStream,
    noise: TransportState,
    buf: Vec<u8>,
}

impl Channel {
    /// Largest payload [`Channel::send`] accepts.
    pub const MAX_PAYLOAD_LEN: usize = MAX_MESSAGE_LEN - TAG_LEN;

    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > Self::MAX_PAYLOAD_LEN {
            return Err(Error::Protocol("message too large"));
        }
        self.buf.resize(MAX_MESSAGE_LEN, 0);
        let len = self.noise.write_message(payload, &mut self.buf)?;
        write_frame(&mut self.stream, &self.buf[..len])
    }

    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let message = read_frame(&mut self.stream)?;
        self.buf.resize(MAX_MESSAGE_LEN, 0);
        let len = self.noise.read_message(&message, &mut self.buf)?;
        Ok(self.buf[..len].to_vec())
    }

    /// Closes the connection in both directions.
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    let len = u16::try_from(payload.len()).map_err(|_| Error::Protocol("frame too large"))?;
    let mut frame = Vec::with_capacity(2 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    stream.write_all(&frame)?;
    Ok(())
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len = [0; 2];
    stream.read_exact(&mut len)?;
    let mut frame = vec![0; u16::from_be_bytes(len).into()];
    stream.read_exact(&mut frame)?;
    Ok(frame)
}

/// Writes the next handshake message and sends it as a frame.
fn send_handshake(
    stream: &mut TcpStream,
    noise: &mut HandshakeState,
    payload: &[u8],
) -> Result<()> {
    let mut buf = vec![0; MAX_MESSAGE_LEN];
    let len = noise.write_message(payload, &mut buf)?;
    write_frame(stream, &buf[..len])
}

/// Reads the next handshake frame, returning its decrypted payload.
fn recv_handshake(stream: &mut TcpStream, noise: &mut HandshakeState) -> Result<Vec<u8>> {
    let message = read_frame(stream)?;
    let mut buf = vec![0; MAX_MESSAGE_LEN];
    let len = noise.read_message(&message, &mut buf)?;
    buf.truncate(len);
    Ok(buf)
}

/// Builds the Noise handshake for either side, in pairing mode when `psk`
/// holds the SPAKE2 key.
fn noise_builder<'a>(
    keypair: &'a Keypair,
    prologue: &'a [u8],
    psk: Option<&'a [u8; 32]>,
) -> Result<snow::Builder<'a>> {
    let params = if psk.is_some() {
        NOISE_PAIR
    } else {
        NOISE_CONNECT
    };
    let mut builder = snow::Builder::new(params.parse()?)
        .local_private_key(&keypair.private)?
        .prologue(prologue)?;
    if let Some(psk) = psk {
        builder = builder.psk(3, psk)?;
    }
    Ok(builder)
}

fn spake2_key(key: Vec<u8>) -> Result<[u8; 32]> {
    key.try_into()
        .map_err(|_| Error::Protocol("unexpected SPAKE2 key length"))
}
