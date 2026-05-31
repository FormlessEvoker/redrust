# RedRust Documentation

Welcome to the RedRust documentation! Here we explain everything from foundational networking concepts to detailed implementation decisions inside the codebase.

## 📖 Foundations
High-level network, OS, and Rust concepts.
- [Connection Lifecycle & Core Concepts](foundations/connection_lifecycle.md): Explains sockets, file descriptors, `TcpStream`, and how they map together.
- [Kernel Socket Mechanics & Polling](foundations/kernel_socket_mechanics.md): Deep dive into kernel receive/send buffers, hardware interrupts, how `poll()` actually works, and the anatomy of a TCP EOF.
- [Poll Flags: The Event Loop Vocabulary](foundations/poll_flags.md): Explains the input and output flags (`POLLIN`, `POLLOUT`, `POLLHUP`, etc.) used in the `poll()` system call.
- [TCP State Machine](foundations/tcp_state_machine.md): Visual map of the connection lifecycle states (`LISTEN`, `ESTABLISHED`, `TIME_WAIT`, etc.) and what they mean.

## ⚙️ Implementation
Details on the specific architectural choices in RedRust.
*(More docs coming soon)*

## 💡 Examples
Code examples and tutorials.
- [Exploring Socket Options (getsockopt)](examples/getsockopt.md): A runnable example of peering into kernel state (buffer sizes) using `nix`.

## 📚 Reference
Protocol specifications and API contracts.
*(More docs coming soon)*

---

*Want to add a new document? Please read our [Contribution Guidelines](CONTRIBUTING_DOCS.md) and use the [Document Template](_template.md).*