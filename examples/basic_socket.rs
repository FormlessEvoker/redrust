use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn run_server() -> std::io::Result<()> {
    // 1. Obtain a socket handle, bind to an address, and listen
    let listener = TcpListener::bind("0.0.0.0:1234")?;
    println!("Server listening on 0.0.0.0:1234...");

    // 2. Accept connections
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // 3. Read & write
                let mut rbuf = [0; 64];
                match stream.read(&mut rbuf) {
                    Ok(n) if n > 0 => {
                        let msg = String::from_utf8_lossy(&rbuf[..n]);
                        println!("client says: {}", msg.trim_end_matches('\0'));

                        let wbuf = b"world\n";
                        stream.write_all(wbuf)?;
                    }
                    Ok(_) => println!("client disconnected before sending data"),
                    Err(e) => println!("read error: {}", e),
                }
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
    }

    Ok(())
}

fn run_client() -> std::io::Result<()> {
    // 1. Obtain a socket handle and connect
    let mut stream = TcpStream::connect("127.0.0.1:1234")?;

    // 2. Write data to the server
    let msg = b"hello\n";
    stream.write_all(msg)?;

    // 3. Read response from the server
    let mut rbuf = [0; 64];
    let n = stream.read(&mut rbuf)?;
    let response = String::from_utf8_lossy(&rbuf[..n]);

    println!("server says: {}", response.trim_end_matches('\0'));

    Ok(())
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 && args[1] == "client" {
        run_client()
    } else {
        println!("Hint: You can run the client by passing 'client' as an argument.");
        println!("Starting server...");
        run_server()
    }
}
