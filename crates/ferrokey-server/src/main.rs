//! Runs on the controlled machine and injects received input events.

fn main() {
    match ferrokey_input::backend() {
        Ok(_backend) => println!("ferrokey-server: input backend ready"),
        Err(err) => {
            eprintln!("ferrokey-server: {err}");
            std::process::exit(1);
        }
    }
}
