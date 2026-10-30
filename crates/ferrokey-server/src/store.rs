//! The server's persistent identity and paired devices, plus the pairing
//! window that lets new devices in.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ferrokey_net::server::Authorizer;
use ferrokey_net::{Keypair, PublicKey};

/// Wrong codes allowed before pairing closes, keeping guessing hopeless.
const MAX_PAIRING_FAILURES: u32 = 3;

pub struct Store {
    devices_path: PathBuf,
    devices: Mutex<HashMap<PublicKey, String>>,
    pairing: Mutex<Option<Pairing>>,
}

struct Pairing {
    code: String,
    failures: u32,
}

impl Store {
    /// Loads the paired devices from `dir`.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let devices_path = dir.join("devices");
        let devices = match fs::read_to_string(&devices_path) {
            Ok(contents) => parse_devices(&contents),
            Err(err) if err.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => return Err(err),
        };
        Ok(Self {
            devices_path,
            devices: Mutex::new(devices),
            pairing: Mutex::new(None),
        })
    }

    pub fn device_count(&self) -> usize {
        self.devices.lock().unwrap().len()
    }

    /// Opens the pairing window with a fresh code, which is returned.
    pub fn start_pairing(&self) -> io::Result<String> {
        let code = random_code()?;
        *self.pairing.lock().unwrap() = Some(Pairing {
            code: code.clone(),
            failures: 0,
        });
        Ok(code)
    }

    fn save_devices(&self, devices: &HashMap<PublicKey, String>) -> io::Result<()> {
        let mut contents = String::new();
        for (key, name) in devices {
            contents.push_str(&format!("{} {name}\n", key.to_hex()));
        }
        write_private(&self.devices_path, contents.as_bytes())
    }
}

impl Authorizer for Store {
    fn pairing_code(&self) -> Option<String> {
        self.pairing
            .lock()
            .unwrap()
            .as_ref()
            .map(|pairing| pairing.code.clone())
    }

    fn pairing_failed(&self) {
        let mut pairing = self.pairing.lock().unwrap();
        if let Some(state) = pairing.as_mut() {
            state.failures += 1;
            if state.failures >= MAX_PAIRING_FAILURES {
                *pairing = None;
                eprintln!(
                    "Too many wrong pairing codes; pairing closed. Restart with --pair to try again."
                );
            }
        }
    }

    fn add_device(&self, key: &PublicKey, name: &str) -> io::Result<()> {
        // The code is single use.
        *self.pairing.lock().unwrap() = None;
        let mut devices = self.devices.lock().unwrap();
        devices.insert(*key, name.to_owned());
        self.save_devices(&devices)
    }

    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.devices.lock().unwrap().contains_key(key)
    }
}

/// Parses `<hex public key> <name>` lines, skipping any it can't read.
fn parse_devices(contents: &str) -> HashMap<PublicKey, String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, name) = line.split_once(' ')?;
            Some((PublicKey::from_hex(key)?, name.to_owned()))
        })
        .collect()
}

/// Loads the server's keypair from `dir`, creating one on first run.
pub fn load_or_create_keypair(dir: &Path) -> io::Result<Keypair> {
    let path = dir.join("server.key");
    match fs::read(&path) {
        Ok(bytes) => Keypair::from_bytes(&bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is corrupt", path.display()),
            )
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let keypair = Keypair::generate().map_err(io::Error::other)?;
            write_private(&path, &keypair.to_bytes())?;
            Ok(keypair)
        }
        Err(err) => Err(err),
    }
}

/// A six-digit code, uniformly random.
fn random_code() -> io::Result<String> {
    // Rejection sampling avoids the bias of a plain modulo.
    const LIMIT: u32 = u32::MAX - u32::MAX % 1_000_000;
    loop {
        let n = getrandom::u32().map_err(io::Error::other)?;
        if n < LIMIT {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

/// Writes a file readable only by the current user, where the OS supports it.
fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(contents)
}

/// `$XDG_CONFIG_HOME/ferrokey` (or `~/.config/ferrokey`) on Unix,
/// `%APPDATA%\ferrokey` on Windows.
pub fn default_dir() -> Option<PathBuf> {
    let non_empty = |var| {
        std::env::var_os(var)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        non_empty("APPDATA")?
    } else {
        non_empty("XDG_CONFIG_HOME").or_else(|| Some(non_empty("HOME")?.join(".config")))?
    };
    Some(base.join("ferrokey"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..100 {
            let code = random_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn device_names_with_spaces_survive_parsing() {
        let key = PublicKey([7; 32]);
        let devices = parse_devices(&format!("{} Pixel 9 Pro\nnot a device\n", key.to_hex()));
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[&key], "Pixel 9 Pro");
    }
}
