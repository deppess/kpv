# tpv

A PDF viewer for terminals with the kitty graphics protocol, such as kitty and Rex. Pages scroll continuously in a framed window with a status bar, and you get vim-like keys, page jumps, full-text search with highlights, a table of contents, a key list on `?`, clickable links and live reload when the file changes. It is a fork of [MeowPDF](https://github.com/monoamine11231/MeowPDF), reworked for one setup.

Source: [Codeberg](https://codeberg.org/deppes/tpv) (primary) · [GitHub](https://github.com/deppess/tpv) (mirror)

## Usage

```sh
tpv <file.pdf>
```

It needs a terminal with the kitty graphics protocol, such as kitty 0.31 or later or Rex. It does not work inside tmux.

The file name sits in the top edge of the frame. The bottom edge is the status bar:

```
╰ NOR │ 12/177 │ 110% │ /zettel 2/374 ───╯
```

From left to right it shows the mode (or the `:` / `/` prompt), the page and the page count, the zoom (100% fits the page width), a pending count, the search and its match number, short messages, where a hovered link points, and the section you're reading, such as `Methods › Setup`.

## Keys

| Key | Action |
|---|---|
| `j` `k` `h` `l` / arrows | Move |
| `J` / `K`, `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | Next / previous page |
| `Ctrl-d` / `Ctrl-u` | Half a screen down / up |
| `gg` / `G` | First / last page |
| `:45`, `45G`, `45gg` | The 45th page |
| `o`, `Tab` | Table of contents |
| `/text` then `Enter` | Search the whole document |
| `n` / `N` | Next / previous match (wraps around) |
| `Backspace`, `Esc`, `:noh` | Clear the search |
| `+` / `-` | Zoom in / out |
| `=` / `z` | Fit the page width (100%) / fit the whole page |
| `C` | Center the page |
| `Ctrl-r` | Dark pages: paper and text in the theme's `pages.dark_background` and `pages.dark_text`, colors keeping their hue |
| `?` | Every key from your config, grouped, with the keys of the contents panel and the `:` commands; `j`/`k`, `d`/`u`, `g`/`G` and the mouse wheel scroll, `q` closes |
| `q`, `:q` | Quit |

A number before a key repeats it: `3 Ctrl-f` moves three pages, and `5j` moves five steps. In a `:` or `/` prompt, `Esc` or backspacing past the start cancels, and `Ctrl-u` clears the line.

Clicking a link opens it, or jumps to the spot it points to when it points into the document.

## Remembered position

Each file opens as you left it: the same page, the same spot on it, the same zoom and the same spot across a wide page, with the contents panel open or closed as it was. All of it is written a moment after the view stops changing, on quitting, and when the terminal window closes or tpv is stopped with `kill`. A position is kept per page rather than in pixels, so it comes back right in a window of another size.

Each file gets one small TOML file in `~/.local/state/tpv/` (or under `$XDG_STATE_HOME`). A file is found by the permanent ID inside the PDF, so moving or renaming it keeps its place, and by its path, for PDFs without an ID and for LaTeX output that gets a new one on every build. Files are replaced in one step, so a crash or two viewers on one file never leave a broken one, and a broken or unknown file is ignored. `remember_position = false` turns it off.

## Table of contents

`o` or `Tab` opens the document's outline in a panel over the left side of the page. It opens on the section you're reading, with the sections above it unfolded.

| Key | In the panel |
|---|---|
| `j` `k` / arrows, `Ctrl-d` `Ctrl-u`, `g` `G` | Move |
| `l` / `Right` | Unfold, or step into an unfolded entry |
| `h` / `Left` | Fold, or step out to the parent entry |
| `za`, `zM`, `zR` | Fold or unfold one entry, fold all, unfold all |
| `/text` | Keep only the entries that contain the text, and their parents |
| `+` / `-` | Wider / narrower; the width is kept for next time |
| `Enter` | Go there and close the panel |
| `Space` | Go there and keep the panel open |
| `Esc`, `q`, `o`, `Tab` | Clear the filter, then close |

The mouse wheel scrolls the panel. Clicking an entry goes there, and clicking its `▸` or `▾` folds it.

## Search

Search ignores case. It runs alongside the pages, so they keep drawing and scrolling while it does: the status bar counts the matches found so far, the matches are highlighted as they turn up, and once it's done it goes to the first match from where you are. All matches are highlighted in yellow, the current one in orange, and the search runs again when the file reloads.

## Configuration

The config is created on first run at `~/.config/tpv/config.toml` (or under `$XDG_CONFIG_HOME` when that is set), with a line about each setting. It lists the defaults; a setting or key left out keeps its default, so the file only needs what you change, and settings that later versions add work without touching it:

- `[viewer]`: `scroll_step`, `zoom_step` (percent of the page width), `resolution` (pages are drawn at the screen's own resolution times this, again whenever the zoom changes, so they stay sharp when zoomed in; zoomed far in, the part in view is drawn sharp and the pages around it coarser until you get to them, so any zoom fits in the memory the terminal keeps for images), `page_gap`, `invert_wheel`, `link_hints` (where a hovered link points) and `remember_position`.
- `[bindings]`: `"key" = "Action"`, added to the default keys; `"key" = ""` unbinds a default. Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md). The actions are `MoveUp`, `MoveDown`, `MoveLeft`, `MoveRight`, `NextPage`, `PrevPage`, `JumpFirstPage`, `JumpLastPage`, `CommandLine`, `Search`, `NextMatch`, `PrevMatch`, `ClearSearch`, `ShowOutline`, `HalfPageDown`, `HalfPageUp`, `ZoomIn`, `ZoomOut`, `FitWidth`, `FitPage`, `CenterViewer`, `ToggleInverse`, `ShowHelp` and `Quit`.

## Colors

Colors are in `theme.toml` next to the config, written on first run with every color and what it's for. A color can be `"#1e2030"`, `"#fff"`, `"rgb(30, 32, 48)"`, `"hsl(230, 23%, 15%)"`, a palette number like `8`, a name like `"cyan"` or `"bright-black"`, or `"default"` for the terminal's own color.

Panel backgrounds (`contents.background`, `contents.selected`, `help.background`) and the colors painted on pages (`pages.match`, `pages.current_match`) can be see-through: add two hex digits of opacity, from `00` (clear) to `ff` (solid), as in `"#1e2030cc"`. The page then shows through the panel. A page color without them is mixed in at a strength that keeps the text readable.

A mistake in the file doesn't stop tpv: that color keeps its default, and the status bar says what's wrong.

## Using it from yazi

```toml
[[opener.tpv]]
run = 'tpv %s'
desc = "tpv"
block = true

[open]
rules = [
  { url = "*.pdf", use = ["tpv"] },
]
```

## Build and test

tpv is built from source, and built and tested on macOS. It needs Rust, and the Xcode command line tools for building MuPDF. Install it from a checkout, so that `.cargo/config.toml` leaves out the parts of MuPDF a PDF viewer doesn't use (`cargo install --git` doesn't read it, and builds a binary about 2 MB larger):

```sh
git clone https://codeberg.org/deppes/tpv && cd tpv
cargo install --path .
```

From a checkout, what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D clippy::all
cargo test --all-features -- --test-threads 1
cargo build --release
cargo install --path .
```

The tests cover the page placement and clipping math, which images a frame places again, reading the outline from a PDF, folding and filtering the contents panel, the remembered state (moved, rebuilt and broken files), key and prompt handling, search stepping, the help list, reading colors and the theme file, laying the config over the defaults, and the file watcher, which must reload once when a file is replaced by a rename. Drawing in the terminal itself is checked by hand.

## License

PolyForm Strict License 1.0.0 (`SPDX-License-Identifier: PolyForm-Strict-1.0.0`). See [LICENSE](LICENSE). The code inherited from MeowPDF stays under its MIT license, kept in [LICENSE-MeowPDF](LICENSE-MeowPDF). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
