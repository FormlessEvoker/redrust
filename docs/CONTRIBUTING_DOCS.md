# Contributing to RedRust Documentation

We believe that good software requires good documentation. As this repository grows, having a structured approach to documentation ensures concepts remain approachable for both new learners and contributors.

## Documentation Structure

Our documentation is separated by audience and intent:
- **`foundations/`**: High-level networking and protocol concepts (e.g., sockets, file descriptors, TCP lifecycle). Agnostic to the exact Rust implementation.
- **`implementation/`**: How we implemented specific parts of RedRust (e.g., the `Conn` struct, the event loop, state machines).
- **`examples/`**: Runnable code examples illustrating how to use components of the library.
- **`reference/`**: Detailed API contracts and specifications.

## Authoring Guidelines
1. **Use the Template**: Start new documents by copying `docs/_template.md`.
2. **Visuals Matter**: Use Mermaid.js diagrams (sequence diagrams, flowcharts) to explain abstract ideas wherever possible.
3. **Keep it Accessible**: Assume the reader is a competent programmer but might not know the depths of kernel networking or Rust async semantics. Use plain language.
4. **Link Generously**: Link to other docs in the repository to build a web of knowledge.

## Review Checklist
Before submitting a documentation PR, verify:
- [ ] Diagram syntax is valid (renders correctly in GitHub/VS Code).
- [ ] Code examples compile and are accurate to the current API surface.
- [ ] Status flag is set appropriately (e.g., Draft vs. Reviewed).
- [ ] Added a link to the new doc in `docs/README.md`.
