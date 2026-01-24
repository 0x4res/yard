# YARD

**Yet Another Rust Desktop** — The Wayland RDP client that doesn't suck.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![Wayland](https://img.shields.io/badge/wayland-native-green.svg)](https://wayland.freedesktop.org/)

---

## Why YARD?

Ever tried using RDP on Linux with multiple monitors? Yeah, it doesn't work.

- **xFreeRDP** crashes on Hyprland
- **wlfreerdp** is deprecated and broken
- **Remmina** inherits all FreeRDP's problems

YARD is built from the ground up for **Wayland** — not ported from X11, not a wrapper around broken C code. Pure Rust, native Wayland, multi-monitor fullscreen that actually works.

---

## Features

### Core
- **Multi-monitor fullscreen** — All your screens, in fullscreen, at the right resolution. Finally.
- **Native Wayland** — Works on Hyprland, Sway, and other wlroots compositors
- **Zero crashes** — Rust memory safety, no segfaults
- **Low latency** — Optimized for daily remote work

### Planned for v1.0
- [ ] RDP protocol (via IronRDP)
- [ ] TLS + NLA authentication
- [ ] Multi-monitor detection & fullscreen
- [ ] Minimal overlay (hover at top edge)
- [ ] Audio output & input (PipeWire)
- [ ] Clipboard sync (text & files)
- [ ] Keyboard shortcut to exit fullscreen

### Future
- [ ] Hardware video decode (Vulkan/VA-API)
- [ ] USB device redirection
- [ ] RD Gateway support

---

## Installation

### From source (recommended for now)

```bash
# Clone the repository
git clone https://github.com/0x4res/yard.git
cd yard

# Build
cargo build --release

# Install (optional)
cargo install --path crates/yard-client
```

### Dependencies

**Arch Linux:**
```bash
sudo pacman -S wayland libxkbcommon pipewire
```

**Debian/Ubuntu:**
```bash
sudo apt install libwayland-dev libxkbcommon-dev libpipewire-0.3-dev
```

---

## Usage

### Basic connection

```bash
yard connect your-server.example.com
```

### Fullscreen on all monitors

```bash
yard connect your-server.example.com --fullscreen --all-monitors
```

### With credentials

```bash
yard connect your-server.example.com -u username -d DOMAIN
```

### Using a saved profile

```bash
yard connect work
```

### Help

```bash
yard --help
yard connect --help
```

---

## Configuration

YARD uses a simple TOML configuration file at `~/.config/yard/config.toml`:

```toml
[profiles.work]
server = "rdp.company.com"
username = "john.doe"
domain = "CORP"
fullscreen = true
all_monitors = true

[profiles.home]
server = "192.168.1.100"
username = "admin"
fullscreen = true
```

---

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+Alt+Enter` | Toggle fullscreen |
| `Ctrl+Alt+End` | Disconnect |

The overlay appears when you hover at the top edge of the screen.

---

## Why Rust? Why Wayland-native?

### The Problem

Every existing Linux RDP client was written for X11 and poorly ported to Wayland. They all share the same issues:

1. **Multi-monitor doesn't work** — At best, you get one stretched window
2. **Crashes on wlroots** — Segfaults on Hyprland, Sway, etc.
3. **No proper overlay** — Get stuck in fullscreen with no way out
4. **Written in C** — Memory bugs, security issues

### The Solution

YARD is different:

- **Wayland-first** — Designed for Wayland from day one, not ported
- **Per-monitor windows** — Each monitor gets its own window (works with tiling WMs)
- **Rust** — Memory safe, no segfaults, predictable performance
- **Modern stack** — IronRDP, PipeWire, wlr-output-management

---

## Architecture

```
yard/
├── crates/
│   ├── yard-core/      # Domain logic, state machines (no I/O)
│   ├── yard-protocol/  # RDP protocol handling via IronRDP
│   ├── yard-wayland/   # Wayland integration, multi-monitor
│   ├── yard-video/     # Video decoding (FFmpeg)
│   ├── yard-audio/     # PipeWire audio integration
│   └── yard-client/    # CLI application
└── ...
```

Built with [Hexagonal Architecture](https://en.wikipedia.org/wiki/Hexagonal_architecture_(software)) for testability and modularity.

---

## Contributing

Contributions are welcome! Please read our contributing guidelines before submitting PRs.

### Development setup

```bash
# Clone
git clone https://github.com/0x4res/yard.git
cd yard

# Build
cargo build

# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run -- connect your-server.com
```

### Good first issues

Look for issues labeled `good first issue` — they're great starting points for new contributors.

---

## Roadmap

### v1.0 — "Daily Driver"
Everything needed to replace the Windows RDP client for daily use:
- Multi-monitor fullscreen
- Audio (output + microphone)
- Clipboard
- Stable, no crashes

### v1.1 — Performance
- Hardware video decode
- USB device redirection

### v2.0 — Ecosystem
- GUI configuration app
- RemoteApp support
- Plugin system

---

## FAQ

**Q: Does YARD work on X11?**
A: No. YARD is Wayland-only by design. Use FreeRDP for X11.

**Q: Which compositors are supported?**
A: Any Wayland compositor that supports `wlr-output-management` and `xdg-shell`. Tested on Hyprland and Sway.

**Q: Can I connect to Windows machines?**
A: Yes! YARD implements standard RDP protocol and works with Windows 10/11 and Windows Server.

**Q: Is it production-ready?**
A: Not yet. We're working toward v1.0.

---

## Acknowledgments

- [IronRDP](https://github.com/Devolutions/IronRDP) — Rust RDP protocol implementation
- [Smithay](https://github.com/Smithay/smithay) — Wayland client libraries
- [FreeRDP](https://github.com/FreeRDP/FreeRDP) — Reference for RDP protocol details

---

## License

MIT License — see [LICENSE](LICENSE) for details.

---

**YARD** — Because your monitors deserve better.
