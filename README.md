# Zed Playdate Extension

A [Zed](https://zed.dev) extension that provides language support and debugging capabilities for [Playdate](https://play.date) game development.

## Features

- **Language Support** for Playdate-specific file formats:
  - Playdate Lua with enhanced syntax highlighting
  - PDXInfo manifest files
  - Animation.txt files
- **Debug Adapter** integration for the Playdate Simulator
- **Tree-sitter grammars** for accurate parsing and syntax highlighting

## Installation

### From Zed Extensions

1. Open Zed
2. Open the command palette (`Cmd+Shift+P` on macOS, `Ctrl+Shift+P` on Linux/Windows)
3. Search for "extensions"
4. Find "Playdate" and click Install

### From Source

1. Clone this repository:
   ```bash
   git clone https://github.com/subpop/zed-playdate.git
   cd zed-playdate
   ```
2. Open Zed
3. Open the Extensions Browser (`Cmd+Shift+X` on macOS, `Ctrl+Shift+X` on Linux/Windows)
4. Click "Install Dev Extension"
5. Select the `zed-playdate` directory

## Development

This extension is built with Rust and uses the Zed Extension API.

### Prerequisites

- Rust toolchain with `wasm32-wasip2` target
- Zed editor

### Building

```bash
cargo build --release --target wasm32-wasip2
```

Install [From Source](#from-source).

## License

MIT License - see [LICENSE](LICENSE) for details.

## Author

Link Dupont <link@sub-pop.net>

## Repository

https://github.com/subpop/zed-playdate
