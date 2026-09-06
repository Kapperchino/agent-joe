# Agent standards

- Prefer a functional approach whenever possible
- Use explicit state machines for stateful workflows.
- Avoid nested if else statements whenever possible, replace with concrete states represented by enums.
- No early returns, except propagation with `?`.
- No `ensure!`, `break`, or code comments.
- Use Rust structs and enums to represent domain concepts; prefer named types over tuple-based interfaces.
- Put validation in the relevant type or its constructor. Use new types where useful without overdoing wrappers.
  - Avoid things like fn validate(...) → anyhow::Result<()>, instead use a new type
- Use typed errors only when callers need to distinguish them for handling.
- Code you write should be as simple as possible, avoid complications