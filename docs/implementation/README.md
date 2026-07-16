# Implementation Docs

These documents describe the current learning-oriented implementation and its
tradeoffs. They complement the conceptual material in `docs/foundations/`.

- [`Conn` and connection handling](conn.md): per-connection state, framing,
  pipelining, backpressure, and half-close behavior.
- [`Why nix`](why_nix.md): rationale for using low-level polling APIs.
- [`Pipelining and buffering hardening`](pipelining_future_hardening.md):
  deliberately deferred limits, fairness, and scalability improvements.

The protocol and implementation will continue to change as later chapters add
Redis command parsing and RESP framing.
