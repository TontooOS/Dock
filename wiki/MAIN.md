# Tontoo Dock -- Wiki

Tontoo Dock is the system dock of TontooOS: a macOS Sonoma / Sequoia-accurate
glass bar (frosted glass, bounce, running dots, separator) built with
TontooUIKit, TontooUI, CoreIcon and GTK4. It has no traffic lights and floats
bottom-center with click-through outside. Hover magnification is disabled.

- Repository: tontoo-os/TontooProgramms/Dock
- License: TCL v26.1
- Version: 26.1.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| Dock | [Dock.md](Dock.md) | Glass dock: dots, bounce, separator, HiDPI, lang (magnification removed) |

## Quick Start

```bash
wsl -d archlinux -- bash -lc 'cd /mnt/c/Users/arlo1/Documents/TontooProgramms/Dock && cargo build'
./target/debug/dock
DOCK_APPEARANCE=light ./target/debug/dock  # force light
```

The dock opens as a frosted glass panel bottom-center (gap 8 px). Icons stay
at fixed size (hover magnification removed), running apps show a centered dot,
clicks bounce 2–3 times, and the vertical separator divides apps from system
items.

See [Dock.md](Dock.md) for details.
