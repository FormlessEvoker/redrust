use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

// Ask the OS for any free localhost port, then immediately release it.
// The test uses that port a moment later when spawning the server process.
fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// Launch the real server binary as a child process with a test-specific PORT.
// We discard stdout/stderr here to keep test output quiet unless the test fails.
fn spawn_server(port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_redrust"))
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn redrust server")
}

// The child process may need a short moment to bind and start polling.
// Retry the TCP connection a few times instead of assuming instant readiness.
fn connect_when_ready(port: u16) -> TcpStream {
    for _ in 0..50 {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }

    panic!("server never became ready on port {}", port);
}

// Best-effort cleanup so the server process does not outlive the test.
// `kill` may fail if the process already exited, which is fine here.
fn kill_server(server: &mut Child) {
    let _ = server.kill();
    let _ = server.wait();
}

#[test]
fn echoes_multiple_clients() {
    // Pick an available port first so the test can run without hardcoded ports.
    let port = reserve_port();
    // Start the actual binary we would run manually with `cargo run`.
    let mut server = spawn_server(port);

    let test_result = (|| {
        // Connect two independent clients to exercise more than one live socket.
        let mut client_a = connect_when_ready(port);
        let mut client_b = connect_when_ready(port);

        // Each client sends a distinct payload so we can verify echoes separately.
        client_a.write_all(b"ping")?;
        client_b.write_all(b"pong")?;

        let mut a_buf = [0u8; 4];
        let mut b_buf = [0u8; 4];

        // Read back exactly one echoed message from each client.
        client_a.read_exact(&mut a_buf)?;
        client_b.read_exact(&mut b_buf)?;

        assert_eq!(&a_buf, b"ping");
        assert_eq!(&b_buf, b"pong");

        // Dropping the clients closes the TCP connections from the client side.
        drop(client_a);
        drop(client_b);

        Ok::<(), std::io::Error>(())
    })();

    // Always tear the server down, even if one of the assertions above failed.
    kill_server(&mut server);
    test_result.unwrap();
}
