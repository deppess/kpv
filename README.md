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

From left to right it shows the mode (or the `:` / `/` prompt), the page, the zoom (100% fits the page width), a pending count, the search and its match number, short messages, where a hovered link points, and the section you're reading, such as `Methods › Setup`.

## Keys

| Key | Action |
|---|---|
| `j` `k` `h` `l` / arrows | Move |
| `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | Next / previous page |
| `gg` / `G` | First / last page |
| `:45`, `:xiv` | The page printed as 45 or xiv |
| `:#45`, `45G`, `45gg` | The 45th page of the file |
| `Ctrl-o` / `Ctrl-n` | Back to where the last jump started / forward again |
| `ma` / `` `a `` or `'a` | Set mark a / go to it (any letter a to z) |
| ` `` ` | Back to where the last jump started |
| `o`, `Tab` | Table of contents |
| `/text` then `Enter` | Search the whole document |
| `n` / `N` | Next / previous match (wraps around) |
| `Backspace`, `Esc`, `:noh` | Clear the search |
| `+` / `-` | Zoom in / out |
| `C` | Center the page |
| `Ctrl-r` | Invert colors |
| `Ctrl-a` | Transparent page background |
| `?` | List every key from your config; `j`/`k`, `d`/`u`, `g`/`G` scroll, `q` closes |
| `q`, `:q` | Quit |

Marks belong to the file and are remembered with its position. `:marks` lists them with their pages, `:delm a` deletes mark a and `:delm!` deletes them all.

Jumps are page jumps (`gg`, `G`, `:`), marks, links, table of contents entries and search matches. `Ctrl-o` walks back through where they started and `Ctrl-n` walks forward again, as `Ctrl-o` and `Ctrl-i` do in vim. `Ctrl-i` can't be used, because terminals send it as `Tab`.

A number before a key repeats it: `3 Ctrl-f` moves three pages, and `5j` moves five steps. In a `:` or `/` prompt, `Esc` or backspacing past the start cancels, and `Ctrl-u` clears the line.

Page numbers follow the book. When the PDF carries printed page numbers (roman numerals for the front matter, a restart at chapter one, prefixes like `A-3`), the status bar shows them as `xiv (14/412)`: the printed number, then the position in the file. A `:` number that no page is printed with goes to that position instead, and the status bar says so. For a scan without printed page numbers, `:offset 12` makes the 13th page printed page 1, and `:offset` shows the current offset. Clicking a link opens it, or jumps to the spot it points to when it points into the document.

## Remembered position

Each file opens where you left it: the same page, the same spot on it and the same zoom, along with its marks and page offset. The position is written two seconds after the view stops moving and again on quitting, so closing the terminal window doesn't lose it. A position is kept per page rather than in pixels, so it comes back right in a window of another size.

Each file gets one small TOML file under `~/Library/Application Support/viewmer/state/` (macOS) or `~/.local/state/viewmer/` (Linux). A file is found by the permanent ID inside the PDF, so moving or renaming it keeps its place, and by its path, for PDFs without an ID and for LaTeX output that gets a new one on every build. Files are replaced in one step, so a crash or two viewers on one file never leave a broken one, and a broken or unknown file is ignored. `[state] enabled = false` turns it off.

## Table of contents

`o` or `Tab` opens the document's outline in a panel over the left side of the page. It opens on the section you're reading, with the sections above it unfolded, and the page numbers in it are the printed ones.

| Key | In the panel |
|---|---|
| `j` `k` / arrows, `Ctrl-d` `Ctrl-u`, `g` `G` | Move |
| `l` / `Right` | Unfold, or step into an unfolded entry |
| `h` / `Left` | Fold, or step out to the parent entry |
| `za`, `zM`, `zR` | Fold or unfold one entry, fold all, unfold all |
| `/text` | Keep only the entries that contain the text, and their parents |
| `Enter` | Go there and close the panel |
| `Space` | Go there and keep the panel open |
| `Esc`, `q`, `o`, `Tab` | Clear the filter, then close |

The mouse wheel scrolls the panel. Clicking an entry goes there, and clicking its `▸` or `▾` folds it.

## Search

Search ignores case. All matches are highlighted in yellow, the current one in orange, and the search runs again when the file reloads.

## Configuration

The config is created on first run at `~/Library/Application Support/viewmer/config.toml` (macOS) or `~/.config/viewmer/config.toml` (Linux). New settings and keys from later versions are added to it automatically. A key whose default moved, such as `Ctrl-o` going from inverting colors to jumping back, moves along when the config still has it as shipped; a key you changed stays as you set it.

- `[viewer]`: scroll speed, render precision, memory limit, zoom step and minimum, page gap, preloaded pages, scroll direction.
- `[viewer.uri_hint]`: `enabled` shows where a hovered link points.
- `[state]`: `enabled` remembers the position and marks of every file.
- `[bindings]`: `"key" = "Action"`. Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md). The actions are `MoveUp`, `MoveDown`, `MoveLeft`, `MoveRight`, `NextPage`, `PrevPage`, `JumpFirstPage`, `JumpLastPage`, `JumpBack`, `JumpForward`, `SetMark`, `GoToMark`, `CommandLine`, `Search`, `NextMatch`, `PrevMatch`, `ClearSearch`, `ShowOutline`, `ZoomIn`, `ZoomOut`, `CenterViewer`, `ToggleInverse`, `ToggleAlpha`, `ShowHelp` and `Quit`.

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

The tests cover the page placement and clipping math, reading printed page numbers and the outline from a PDF, folding and filtering the contents panel, the remembered state (moved, rebuilt and broken files), the jump list, marks, moving retired default keys, key and prompt handling, search stepping, the help list, the config merge, and the file watcher, which must reload once when a file is replaced by a rename. Drawing in kitty itself is checked by hand.

## License

MIT, inherited from MeowPDF. See [LICENSE](LICENSE). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
