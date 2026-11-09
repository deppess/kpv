# viewmer

A PDF viewer for the kitty terminal: continuous scrolling, zoom, vim-like keys, mouse and link support, and live reload when the file changes. It is a fork of [MeowPDF](https://github.com/monoamine11231/MeowPDF), reworked for one setup.

## Usage

```sh
viewmer <file.pdf>
```

Requires kitty 0.31 or later. It does not work inside tmux.

| Key | Action |
|---|---|
| `j` `k` `h` `l` / arrows | Move |
| `Ctrl-f` / `Ctrl-b` | Next / previous page |
| `gg` / `G` | First / last page |
| `+` / `-` | Zoom in / out |
| `C` | Center |
| `Ctrl-o` | Invert colors |
| `Ctrl-a` | Transparent page background |
| `q` | Quit |

The config is created on first run at `~/Library/Application Support/viewmer/config.toml` (macOS) or `~/.config/viewmer/config.toml` (Linux). Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md).

## Build and test

```sh
cargo build --release
cargo install --path .
```

## License

MIT, inherited from MeowPDF. See [LICENSE](LICENSE). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
