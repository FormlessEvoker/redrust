fn main() {

  // Steps:
  // 1. create a socket/fd
  // 2. bind to an address
  // 3. listen for incoming connections
  // 4. accept incoming connections
  // 5. read/write data to/from the socket


  // Sample pseudocode:
  // fd = socket()
  // bind(fd, address)
  // listen(fd)
  // while True:
  //     conn_fd = accept(fd)
  //     do_something_with(conn_fd)
  //     close(conn_fd)

    println!("Hello, world!");
}
