# turbo-code

A TUI-based coding assistant powered by Claude. It uses rust-analyzer for project context and provides tools for reading, editing, and checking your Rust code.

## Requirements

- Rust (2024 edition)
- A Claude API key

## Setup

```sh
export CLAUDE_API=<your-claude-api-key>
```

## Build & Run

```sh
cargo run
```

## Keybindings

| Key | Mode | Action |
|-----|------|--------|
| `i` | Normal | Enter edit mode |
| `/` | Normal | Enter command mode |
| `q` | Normal | Quit |
| `h/j/k/l` or arrow keys | Normal | Scroll |
| `G` | Normal | Scroll to bottom (re-enable auto-scroll) |
| `Enter` | Edit/Command | Submit |
| `Esc` | Edit/Command | Back to normal mode |

## Tools

The assistant has access to:

- **ReadFile** - read files from your project
- **StringReplace** - edit files via search-and-replace
- **InsertAfterLine** - insert text after a specific line
- **CargoCheck** - run `cargo check`