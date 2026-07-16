use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_millis(500);

// Ask the OS for an available port. The server binds it immediately after
// spawning, so connect_when_ready handles the small bind/startup window.
fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spawn_server(port: u16) -> ServerGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_redrust"))
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn redrust server");

    ServerGuard { child }
}

fn connect_when_ready(port: u16) -> TcpStream {
    for _ in 0..50 {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
                stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
                return stream;
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }

    panic!("server never became ready on port {port}");
}

fn framed(payload: &[u8]) -> Vec<u8> {
    let mut request = (payload.len() as u32).to_be_bytes().to_vec();
    request.extend_from_slice(payload);
    request
}

// The guard ensures the server is killed even if an assertion or I/O
// operation panics before the test reaches its normal return path.
struct ServerGuard {
    child: Child,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn echoes_multiple_clients() -> Result<(), Box<dyn std::error::Error>> {
    let port = reserve_port();
    let _server = spawn_server(port);

    let mut client_a = connect_when_ready(port);
    let mut client_b = connect_when_ready(port);

    client_a.write_all(&framed(b"ping"))?;
    client_b.write_all(&framed(b"pong"))?;

    let mut a_buf = [0u8; 4];
    let mut b_buf = [0u8; 4];
    client_a.read_exact(&mut a_buf)?;
    client_b.read_exact(&mut b_buf)?;

    assert_eq!(&a_buf, b"ping");
    assert_eq!(&b_buf, b"pong");

    Ok(())
}

#[test]
fn echoes_multiple_pipelined_requests_from_one_client() -> Result<(), Box<dyn std::error::Error>> {
    let port = reserve_port();
    let _server = spawn_server(port);
    let mut client = connect_when_ready(port);

    let payloads = [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()];
    let mut requests = Vec::new();
    for payload in payloads {
        requests.extend_from_slice(&framed(payload));
    }

    client.write_all(&requests)?;

    for expected in payloads {
        let mut response = vec![0; expected.len()];
        client.read_exact(&mut response)?;
        assert_eq!(response, expected);
    }

    Ok(())
}

#[test]
fn echoes_large_request_spanning_multiple_read_iterations() -> Result<(), Box<dyn std::error::Error>>
{
    let port = reserve_port();
    let _server = spawn_server(port);
    let mut client = connect_when_ready(port);

    // Conn::try_read uses a 4096-byte temporary read buffer. This payload
    // requires multiple socket reads before one complete request exists.
    let payload = vec![b'z'; 128 * 1024];
    client.write_all(&framed(&payload))?;

    let mut response = vec![0; payload.len()];
    client.read_exact(&mut response)?;
    assert_eq!(response, payload);

    Ok(())
}
