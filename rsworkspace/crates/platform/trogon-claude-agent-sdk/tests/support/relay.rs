//! A child process that relays its stdio to a test over a Unix socket.
//!
//! The socket path is the first argument; the remaining arguments are the
//! ones the SDK passed to the CLI and are ignored here. Bytes read from
//! stdin are copied to the socket, and the write half is shut down at stdin
//! EOF. The test sends frames made of a tag byte, a big-endian `u32` length
//! and a payload: `o` writes the payload to stdout, `e` writes it to stderr,
//! and `x` exits with the big-endian `i32` code in its payload. The relay
//! exits with code 1 when the socket closes without an exit frame.

#[cfg(unix)]
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
};

#[cfg(unix)]
fn main() {
    std::process::exit(relay().unwrap_or(1));
}

#[cfg(not(unix))]
fn main() {
    std::process::exit(1);
}

#[cfg(unix)]
fn relay() -> std::io::Result<i32> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| std::io::Error::other("the relay socket path argument is missing"))?;
    let mut socket = UnixStream::connect(path)?;
    let mut forward = socket.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut forward);
        let _ = forward.shutdown(Shutdown::Write);
    });
    loop {
        let mut header = [0_u8; 5];
        if socket.read_exact(&mut header).is_err() {
            return Ok(1);
        }
        let [tag, length @ ..] = header;
        let mut payload = vec![0_u8; u32::from_be_bytes(length) as usize];
        socket.read_exact(&mut payload)?;
        match tag {
            b'o' => {
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(&payload)?;
                stdout.flush()?;
            }
            b'e' => {
                let mut stderr = std::io::stderr().lock();
                stderr.write_all(&payload)?;
                stderr.flush()?;
            }
            b'x' => {
                let code: [u8; 4] = payload
                    .try_into()
                    .map_err(|_| std::io::Error::other("exit frame must carry four bytes"))?;
                return Ok(i32::from_be_bytes(code));
            }
            _ => return Err(std::io::Error::other("unknown relay frame")),
        }
    }
}
