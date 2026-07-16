# `Conn` — Per-Connection State

`Conn` owns one non-blocking `TcpStream` and the application state needed to
process it. The event loop owns the connection registry; `Conn` owns socket
I/O, request buffering, response buffering, and read/write interest flags.

## State

- `stream` is the non-blocking TCP socket.
- `incoming` stores bytes read from the socket but not yet parsed.
- `outgoing` stores response bytes waiting to be written.
- `want_read` and `want_write` are translated into `poll(2)` interests by
  `build_poll_args`.
- `peer_closed` records EOF or a peer hangup. It does not immediately close the
  connection because buffered requests and responses still need to drain.

Both byte buffers use [`Buffer`](../../src/buffer.rs), which advances a logical
read position and compacts only when needed.

## Request Processing

The current learning protocol is a four-byte big-endian length followed by the
payload:

```text
00 00 00 05  hello
```

`try_read` drains the non-blocking socket until it reaches `WouldBlock` or EOF;
the `WouldBlock` condition is handled internally as a successful no-more-data
result.
`try_parse_one_request` then consumes exactly one complete frame. If the header
or payload is incomplete, it returns `None` and leaves the input untouched.

The event loop repeats parsing while complete requests are available:

```text
read bytes -> parse request -> queue response -> repeat
```

This handles both a request split across multiple reads and multiple pipelined
requests received in one read.

## Output and Backpressure

`queue_response` appends bytes in request order and enables write readiness.
`try_write` may write only part of the queue, so it consumes only the bytes
accepted by the socket and leaves the remainder for a later `POLLOUT` event.

The outgoing buffer has high and low watermarks:

- At the high watermark, request parsing and `POLLIN` interest are paused.
- Once writing drains the queue to the low watermark, reading is resumed.

This prevents a fast pipelining client from making the server queue output
without bound. The current limits are intentionally fixed constants in
`conn.rs`; configuration and hard request-size limits are future work.

## Connection Shutdown

TCP half-close means the peer has closed its write direction, so the server
observes EOF while it may still need to write a response. The server therefore
marks `peer_closed`, preserves both buffers, and removes the connection only
when `ready_for_close` reports that incoming and outgoing data are empty.

## Related Tests

- Unit tests for buffer and connection behavior are in
  [`src/buffer.rs`](../../src/buffer.rs) and [`src/conn.rs`](../../src/conn.rs).
- End-to-end tests cover multiple clients, pipelined requests, and a request
  spanning multiple read iterations in [`tests/server_echo.rs`](../../tests/server_echo.rs).
