//! Runs on the controlling machine and sends input events to the server.

use ferrokey_protocol::InputEvent;

fn main() {
    let event = InputEvent::MouseMove { dx: 0, dy: 0 };
    println!("ferrokey-client: would send {event:?}");
}
