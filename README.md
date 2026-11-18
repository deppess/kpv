# viewmer

A PDF viewer for the kitty terminal. Pages scroll continuously in a framed window with a status bar, and you get vim-like keys, page jumps, full-text search with highlights, a key list on `?`, clickable links, live reload when the file changes, and a notes panel with your editor beside the pages. It is a fork of [MeowPDF](https://github.com/monoamine11231/MeowPDF), reworked for one setup.

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
| `Ctrl-d` / `Ctrl-u` | Half a screen down / up |
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
| `=` / `z` | Fit the page width (100%) / fit the whole page |
| `C` | Center the page |
| `r` | Reading ruler on / off |
| `Ctrl-r` | Invert colors |
| `Ctrl-a` | Transparent page background |
| `?` | Every key from your config, grouped, with the keys of the panels and the `:` commands; `j`/`k`, `d`/`u`, `g`/`G` and the mouse wheel scroll, `q` closes |
| `Ctrl-h` / `Ctrl-l` | The note / the PDF (see [Notes](#notes)) |
| `Ctrl-Space` | The notes layer: `x` hide the note, `s` switch notes, `c` new note |
| `q`, `:q` | Quit |

Marks belong to the file and are remembered with its position. `:marks` lists them with their pages, `:delm a` deletes mark a and `:delm!` deletes them all.

Jumps are page jumps (`gg`, `G`, `:`), marks, links, table of contents entries and search matches. `Ctrl-o` walks back through where they started and `Ctrl-n` walks forward again, as `Ctrl-o` and `Ctrl-i` do in vim. `Ctrl-i` can't be used, because terminals send it as `Tab`.

A number before a key repeats it: `3 Ctrl-f` moves three pages, and `5j` moves five steps. In a `:` or `/` prompt, `Esc` or backspacing past the start cancels, and `Ctrl-u` clears the line.

Page numbers follow the book. When the PDF carries printed page numbers (roman numerals for the front matter, a restart at chapter one, prefixes like `A-3`), the status bar shows them as `xiv (14/412)`: the printed number, then the position in the file. A `:` number that no page is printed with goes to that position instead, and the status bar says so. For a scan without printed page numbers, `:offset 12` makes the 13th page printed page 1, and `:offset` shows the current offset. Clicking a link opens it, or jumps to the spot it points to when it points into the document.

## Reading ruler

`r` lays a light blue band over the first line of text in view. While it's on, `j` and `k` move it a line down or up (`5j` five lines), on across pages, scrolling when the line nears an edge of the window. Lines side by side, as in two columns, count as one. Other keys and the mouse wheel scroll as usual, and the next `j` or `k` picks the ruler up again at the top of the view. Pages without text, such as scans, have no lines to follow.

## Remembered position

Each file opens where you left it: the same page, the same spot on it and the same zoom, along with its marks and page offset. The position is written two seconds after the view stops moving, on quitting, and when the terminal window closes or viewmer is stopped with `kill`. A position is kept per page rather than in pixels, so it comes back right in a window of another size.

Each file gets one small TOML file in `~/.local/state/viewmer/` (or under `$XDG_STATE_HOME`); files from where earlier versions kept them move there on their own. A file is found by the permanent ID inside the PDF, so moving or renaming it keeps its place, and by its path, for PDFs without an ID and for LaTeX output that gets a new one on every build. Files are replaced in one step, so a crash or two viewers on one file never leave a broken one, and a broken or unknown file is ignored. `remember_position = false` turns it off.

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

## Notes

`Ctrl-h` opens a note beside the PDF, in your editor (helix by default) running in a panel on the left, and the pages move over to the right. The first time, it asks which note: a new one named after the PDF (the title stored in it, or else its file name), one you name, or any note in your vault, which typing filters. A new note starts from your template with a link back to the PDF; the PDF remembers its note, and opens it straight away after that, at its end.

`Ctrl-l` goes back to the PDF and `Ctrl-h` to the note again; a click on either side does the same. While the note has the keys, every key goes to the editor except `Ctrl-h`, `Ctrl-l` and `Ctrl-Space`, which shows the notes layer: `x` hides the note, `s` switches to another note, `c` makes a new one with a name you type. The mouse wheel scrolls whichever side it's over. The contents panel opens beside the note, at the left of the pages.

The note is saved whenever you leave it: going back to the PDF, hiding it, leaving the terminal window, quitting, or closing the window. viewmer starts helix with your own config plus saving on losing focus, and from the vault folder, so its settings for the vault apply. Hiding the note keeps the editor running, so it comes back as you left it; `:q` in the editor closes the panel. If the PDF moves and comes back under its permanent ID, the link in its note moves along.

## Search

Search ignores case. All matches are highlighted in yellow, the current one in orange, and the search runs again when the file reloads.

## Configuration

The config is created on first run at `~/.config/viewmer/config.toml` (or under `$XDG_CONFIG_HOME` when that is set), with a line about each setting. A config and theme from earlier versions, kept in `~/Library/Application Support/viewmer` on macOS, move there on their own:

- `[viewer]`: `scroll_step`, `zoom_step` (percent of the page width), `resolution` (pages are drawn at the screen's own resolution times this, again whenever the zoom changes, so they stay sharp when zoomed in; zoomed far in, the part in view is drawn sharp and the pages around it coarser until you get to them, so any zoom fits in the memory kitty keeps for images), `page_gap`, `invert_wheel`, `link_hints` (where a hovered link points) and `remember_position`.
- `[notes]`: `vault` (the notes folder, where the editor starts), `folder` (where new notes go, inside the vault), `editor`, `template` (`{{title}}` becomes the note's name and `{{source}}` the link to the PDF; without `{{source}}`, the link goes on a `Links:` line or after the title) and `width` (percent of the window).
- `[layers]`: `"keys" = "Name"`. Typing the keys of a layer shows a box with what can follow them.
- `[bindings]`: `"key" = "Action"`. Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md). The actions are `MoveUp`, `MoveDown`, `MoveLeft`, `MoveRight`, `NextPage`, `PrevPage`, `JumpFirstPage`, `JumpLastPage`, `JumpBack`, `JumpForward`, `SetMark`, `GoToMark`, `CommandLine`, `Search`, `NextMatch`, `PrevMatch`, `ClearSearch`, `ShowOutline`, `HalfPageDown`, `HalfPageUp`, `ZoomIn`, `ZoomOut`, `FitWidth`, `FitPage`, `CenterViewer`, `ToggleRuler`, `ToggleInverse`, `ToggleAlpha`, `FocusNote`, `FocusPdf`, `HideNote`, `SwitchNote`, `NewNote`, `ShowHelp` and `Quit`. The notes actions bound with Ctrl or Alt also work while the note has the keys.

A config from an older version is brought up to date when viewmer starts: renamed settings keep their values under the new names, settings that are gone are removed, new ones come in with their explanation, and your own comments and order stay. A key whose default moved, such as `Ctrl-o` going from inverting colors to jumping back, moves along when the config still has it as shipped; a key you changed stays as you set it.

## Colors

Colors are in `theme.toml` next to the config, written on first run with every color and what it's for. A color can be `"#1e2030"`, `"#fff"`, `"rgb(30, 32, 48)"`, `"hsl(230, 23%, 15%)"`, a palette number like `8`, a name like `"cyan"` or `"bright-black"`, or `"default"` for the terminal's own color.

Panel backgrounds (`contents.background`, `contents.selected`, `help.background`, `notes.background`, `notes.selected`, `layer.background`) and the colors painted on pages (`pages.match`, `pages.current_match`, `pages.ruler`) can be see-through: add two hex digits of opacity, from `00` (clear) to `ff` (solid), as in `"#1e2030cc"`. The page then shows through the panel. A page color without them is mixed in at a strength that keeps the text readable.

A mistake in the file doesn't stop viewmer: that color keeps its default, and the status bar says what's wrong.

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

The tests cover the page placement and clipping math, which images a frame places again, reading printed page numbers and the outline from a PDF, folding and filtering the contents panel, the remembered state (moved, rebuilt and broken files), the jump list, marks, finding text lines and placing the ruler, moving retired default keys, key and prompt handling, search stepping, the help list, reading colors and the theme file, encoding keys, the mouse and pastes for the editor, running a program in the notes panel, naming notes and writing them from a template, the note picker, the config merge and upgrade, and the file watcher, which must reload once when a file is replaced by a rename. Drawing in kitty itself is checked by hand.

## License

PolyForm Strict License 1.0.0 (`SPDX-License-Identifier: PolyForm-Strict-1.0.0`). See [LICENSE](LICENSE). The code inherited from MeowPDF stays under its MIT license, kept in [LICENSE-MeowPDF](LICENSE-MeowPDF). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
