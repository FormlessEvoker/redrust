# Pipelining and Buffering: Future Hardening Notes

This document tracks implementation details that are useful once the current event-loop and pipelining work is functionally correct.

The goal is not to add all of this immediately. These are follow-on concerns for a more mature server.

## Scope

These notes apply to:

- per-connection `incoming` buffer growth
- per-connection `outgoing` buffer growth
- fairness and backpressure under pipelined clients
- parser behavior for partial and oversized requests
- buffer data structures under heavy FIFO usage

## 1. Bound `incoming` Growth

The current learning-oriented design can allow `incoming: Vec<u8>` to grow as long as the peer keeps sending bytes.

Reasons to eventually bound it:

- a client can send a huge partial request and never finish it
- a malformed client can exceed reasonable request sizes
- many concurrent connections can cause avoidable memory pressure

Possible future policy:

```rust
const MAX_INCOMING_BYTES: usize = 1 << 20; // example only

if conn.incoming.len() > MAX_INCOMING_BYTES {
    // respond with protocol error or close connection
}
```

Implementation directions:

- enforce a max total buffered bytes per connection
- enforce a max single request size
- reject or close on overflow

## 2. Bound `outgoing` Growth

Pipelining makes `outgoing` more important because a client may send many requests before reading any responses.

Failure mode:

- server keeps accepting requests
- server keeps queueing responses
- client reads too slowly
- `outgoing` grows without bound

Possible future policy:

```rust
const MAX_OUTGOING_BYTES: usize = 1 << 20; // example only

if conn.outgoing.len() > MAX_OUTGOING_BYTES {
    conn.want_read = false; // temporary backpressure
}
```

Implementation directions:

- stop reading when `outgoing` crosses a high-water mark
- resume reading when it drops below a low-water mark
- close the connection if buffered output becomes unreasonable

## 3. Add Backpressure Instead of Always Reading

The simple model is:

- readable event -> read until `WouldBlock`
- parse all complete requests
- queue all responses

That is correct for learning, but not always desirable in a real system.

Future refinement:

- keep `want_read = true` only while the connection is healthy and buffered work is within limits
- temporarily disable reads when:
  - `outgoing` is too large
  - total in-flight work for the connection is too large
  - the server wants fairness across many clients

This turns `want_read` and `want_write` into flow-control tools, not just event-loop toggles.

## 4. Keep Parser Limits Explicit

A pipelining-capable parser should:

- consume one complete request at a time
- leave incomplete trailing bytes in `incoming`
- consume only the parsed prefix, not clear the whole buffer

Future hardening adds size checks:

- incomplete frame prefix too large
- declared payload length too large
- malformed frame lengths

Possible future policy:

```rust
if declared_len > MAX_REQUEST_BYTES {
    // protocol error and close
}
```

## 5. Replace Expensive Front-Draining

If `incoming` or `outgoing` are implemented as `Vec<u8>` and bytes are repeatedly removed from the front, FIFO-heavy workloads can become inefficient.

Why:

- appending to a `Vec` is amortized cheap
- removing from the front shifts the remaining bytes
- many pipelined requests can create repeated copying

Future options:

- keep start/end indexes into a `Vec<u8>`
- compact only when needed
- use `bytes::BytesMut`
- use a ring buffer / deque-like structure

The key improvement is to avoid copying on every consume.

## 6. Add Fairness Limits Per Loop Iteration

A single connection with a large pipeline can monopolize the event loop if the server processes unlimited requests in one turn.

Future refinement:

- process complete requests in a loop
- but optionally cap work per iteration

Examples:

- max requests handled per readable event
- max bytes parsed per readable event
- max bytes written per writable event

This is a throughput vs fairness tradeoff:

- no cap maximizes batching
- caps prevent one client from starving others

## 7. Keep Optimistic Writes as an Optimization

Writing in the same loop iteration as a successful read can save a poll cycle.

That optimization should remain conditional on correct non-blocking semantics:

- attempt the write immediately
- if `WouldBlock`, keep the unwritten bytes in `outgoing`
- continue polling for writable readiness

Important:

- this optimization must not assume the client is already reading
- pipelined clients make that assumption weaker

## 8. Add Tests That Stress Stream Semantics

Good future tests:

- multiple complete requests in one read
- one request split across many reads
- mixed case: several full requests plus one partial trailing request
- very large request spanning many event-loop iterations
- partial writes that require multiple writable events
- slow reader causing `outgoing` backpressure

These tests are valuable because pipelining bugs often come from incorrect assumptions about `read()` or `write()` boundaries.

## 9. Consider Global, Not Just Per-Connection, Limits

Eventually the server may need process-wide resource policies:

- max total buffered bytes across all connections
- max active connections
- max in-flight requests

Per-connection limits are the first step. Global limits are the next step when operational robustness matters.

## Suggested Order for Later Work

When the current implementation is stable, a reasonable follow-up order is:

1. add parser/request-size limits
2. add `incoming` and `outgoing` high-water marks
3. add read backpressure based on buffered output
4. improve FIFO buffer representation
5. add fairness caps per loop iteration
6. add global memory and connection limits

## Non-Goals for Now

These notes should not block the current learning-focused implementation.

For the immediate pipelining work, correctness still comes first:

- read bytes without assuming message boundaries
- parse all complete requests already available
- preserve partial input
- preserve partial output
- write responses in order
