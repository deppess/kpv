# viewmer

A PDF viewer for the kitty terminal. Pages scroll continuously in a framed window with a status bar, and you get vim-like keys, page jumps, full-text search with highlights, a key list on `?`, clickable links, and live reload when the file changes. It is a fork of [MeowPDF](https://github.com/monoamine11231/MeowPDF), reworked for one setup.

## Usage

```sh
viewmer <file.pdf>
```

It needs kitty 0.31 or later, because pages are drawn with kitty's graphics protocol. It does not work inside tmux.

The file name sits in the top edge of the frame. The bottom edge is the status bar:

```
╰ NOR │ 12/177 │ 110% │ /zettel 2/374 ───╯
```

From left to right it shows the mode (or the `:` / `/` prompt), the page, the zoom (100% fits the page width), a pending count, the search and its match number, short messages, and where a hovered link points.

## Keys

| Key | Action |
|---|---|
| `j` `k` `h` `l` / arrows | Move |
| `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | Next / previous page |
| `gg` / `G` | First / last page |
| `:45`, `:xiv` | The page printed as 45 or xiv |
| `:#45`, `45G`, `45gg` | The 45th page of the file |
| `/text` then `Enter` | Search the whole document |
| `n` / `N` | Next / previous match (wraps around) |
| `Backspace`, `Esc`, `:noh` | Clear the search |
| `+` / `-` | Zoom in / out |
| `C` | Center the page |
| `Ctrl-o` | Invert colors |
| `Ctrl-a` | Transparent page background |
| `?` | List every key from your config; `j`/`k`, `d`/`u`, `g`/`G` scroll, `q` closes |
| `q`, `:q` | Quit |

A number before a key repeats it: `3 Ctrl-f` moves three pages, and `5j` moves five steps. In a `:` or `/` prompt, `Esc` or backspacing past the start cancels, and `Ctrl-u` clears the line.

Page numbers follow the book. When the PDF carries printed page numbers (roman numerals for the front matter, a restart at chapter one, prefixes like `A-3`), the status bar shows them as `xiv (14/412)`: the printed number, then the position in the file. A `:` number that no page is printed with goes to that position instead, and the status bar says so.

Search ignores case. All matches are highlighted in yellow, the current one in orange, and the search runs again when the file reloads. Clicking a link opens it, or jumps to the spot it points to when it points into the document.

## Configuration

The config is created on first run at `~/Library/Application Support/viewmer/config.toml` (macOS) or `~/.config/viewmer/config.toml` (Linux). New settings and keys from later versions are added to it automatically.

- `[viewer]`: scroll speed, render precision, memory limit, zoom step and minimum, page gap, preloaded pages, scroll direction.
- `[viewer.uri_hint]`: `enabled` shows where a hovered link points.
- `[bindings]`: `"key" = "Action"`. Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md). The actions are `MoveUp`, `MoveDown`, `MoveLeft`, `MoveRight`, `NextPage`, `PrevPage`, `JumpFirstPage`, `JumpLastPage`, `CommandLine`, `Search`, `NextMatch`, `PrevMatch`, `ClearSearch`, `ZoomIn`, `ZoomOut`, `CenterViewer`, `ToggleInverse`, `ToggleAlpha`, `ShowHelp` and `Quit`.

## Using it from yazi

```toml
[[opener.viewmer]]
run = 'viewmer %s'
desc = "viewmer"
block = true

[open]
rules = [
  { url = "*.pdf", use = ["viewmer"] },
]
```

## Build and test

```sh
cargo build --release
cargo install --path .
cargo fmt --all -- --check
cargo clippy --all-features -- -D clippy::all
cargo test --all-features -- --test-threads 1
```

The tests cover the page placement and clipping math, reading printed page numbers from a PDF, key and prompt handling, search stepping, the help list, the config merge, and the file watcher, which must reload once when a file is replaced by a rename. Drawing in kitty itself is checked by hand.

## License

MIT, inherited from MeowPDF. See [LICENSE](LICENSE). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
