# viewmer

A PDF viewer for the kitty terminal. Pages scroll continuously in a framed window with a status bar, and you get vim-like keys, page jumps, full-text search with highlights, a key list on `?`, clickable links, live reload when the file changes, and a notes panel with your editor beside the pages. It is a fork of [MeowPDF](https://github.com/monoamine11231/MeowPDF), reworked for one setup.

Source: [Codeberg](https://codeberg.org/deppes/viewmer) (primary) · [GitHub](https://github.com/deppess/viewmer) (mirror)

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
| `J` / `K`, `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | Next / previous page |
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
| `Ctrl-r` | Dark pages: paper and text in the theme's `pages.dark_background` and `pages.dark_text`, colors keeping their hue |
| `Ctrl-a` | Transparent page background |
| `?` | Every key from your config, grouped, with the keys of the panels and the `:` commands; `j`/`k`, `d`/`u`, `g`/`G` and the mouse wheel scroll, `q` closes |
| `Ctrl-h` | Open the notes terminal; from inside it, hide it (see [Notes](#notes)) |
| `Ctrl-l` | Back to the PDF |
| `Alt-t` / `Alt-g` | Tag this page in the note / go to the page tagged there |
| `Ctrl-=` / `Ctrl--` | The note wider / narrower, and the pages narrower / wider; kept for next time |
| `q`, `:q` | Quit |

Marks belong to the file and are remembered with its position. `:marks` lists them with their pages, `:delm a` deletes mark a and `:delm!` deletes them all.

Jumps are page jumps (`gg`, `G`, `:`), marks, links, table of contents entries and search matches. `Ctrl-o` walks back through where they started and `Ctrl-n` walks forward again, as `Ctrl-o` and `Ctrl-i` do in vim. `Ctrl-i` can't be used, because terminals send it as `Tab`.

A number before a key repeats it: `3 Ctrl-f` moves three pages, and `5j` moves five steps. In a `:` or `/` prompt, `Esc` or backspacing past the start cancels, and `Ctrl-u` clears the line.

Page numbers follow the book. When the PDF carries printed page numbers (roman numerals for the front matter, a restart at chapter one, prefixes like `A-3`), the status bar shows them as `xiv (14/412)`: the printed number, then the position in the file. A `:` number that no page is printed with goes to that position instead, and the status bar says so. A PDF that doesn't say how its pages are numbered is read for the numbers printed in the pages' headers and footers: when enough pages agree, the pages are numbered as printed (the status bar says so), and when they don't, pages go by their place in the file. `:offset 12` sets it by hand, making the 13th page printed page 1, which then stays; `:offset auto` goes back to reading it off the pages, and `:offset` shows the current offset. A page before printed page 1 goes by its place in the file, as `#3`. Clicking a link opens it, or jumps to the spot it points to when it points into the document.

## Reading ruler

`r` lays a light blue band over the first line of text in view. While it's on, `j` and `k` move it a line down or up (`5j` five lines), on across pages, scrolling when the line nears an edge of the window. Lines side by side, as in two columns, count as one. Other keys and the mouse wheel scroll as usual, and the next `j` or `k` picks the ruler up again at the top of the view. Pages without text, such as scans, have no lines to follow.

## Remembered position

Each file opens as you left it: the same page, the same spot on it, the same zoom and the same spot across a wide page, with the note open and the contents panel and reading ruler as they were, along with its marks, its note and its page offset. All of it is written a moment after the view stops changing, on quitting, and when the terminal window closes or viewmer is stopped with `kill`. A position is kept per page rather than in pixels, so it comes back right in a window of another size.

Each file gets one small TOML file in `~/.local/state/viewmer/` (or under `$XDG_STATE_HOME`). A file is found by the permanent ID inside the PDF, so moving or renaming it keeps its place, and by its path, for PDFs without an ID and for LaTeX output that gets a new one on every build. Files are replaced in one step, so a crash or two viewers on one file never leave a broken one, and a broken or unknown file is ignored. `remember_position = false` turns it off.

## Crashes

viewmer runs under a small watcher. If the viewer crashes, the watcher gives the terminal back, writes what happened to `~/.local/state/viewmer/crash.log` and starts it again, and it opens as it was, from the layout it keeps saved; the status bar says it started again. After three crashes in a minute it stops trying. The note in the notes panel is saved a second after typing stops as well as on leaving it, so a crash loses next to nothing of it either. Saved files are written in full to the disk before they replace the old ones, so a crash or a power cut never leaves half a file.

## Table of contents

`o` or `Tab` opens the document's outline in a panel over the left side of the page. It opens on the section you're reading, with the sections above it unfolded, and the page numbers in it are the printed ones.

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

## Notes

`Ctrl-h` opens a terminal beside the PDF, edge to edge down the left of the window, and the PDF moves over to the right in its frame, which is drawn bright while the PDF has the keys. It runs your shell, the one you log in with, starting in the notes folder, and anything runs in it as in any terminal: helix, yazi, git. It keeps running while hidden, so it comes back as you left it; `exit` closes the panel.

`Ctrl-l` goes back to the PDF and `Ctrl-h` to the terminal again, and `Ctrl-h` from inside the terminal hides it; a click on either side switches too. While the terminal has the keys, every key goes to it except `Ctrl-h`, `Ctrl-l`, `Ctrl-=` and `Ctrl--` (the panel wider and narrower), `Alt-t` and `Alt-g`. On a Mac, Alt is Option, which kitty passes on as Alt with `macos_option_as_alt yes` in its config. The mouse wheel scrolls whichever side it's over. The contents panel opens beside the terminal, at the left of the pages.

Images that programs there draw with kitty's graphics, such as yazi's previews, show in the panel too. It keeps 5,000 lines of what scrolled off: the wheel or `Shift-PageUp` and `Shift-PageDown` scroll back, unless the program takes the mouse or the whole screen itself, and typing goes back to the bottom. A copy a program makes the way terminals pass it on (OSC 52, as helix over ssh, tmux and nvim do) lands on the clipboard. viewmer adds two commands to the terminal. `hx` there is your helix with saving when focus leaves the panel, and a second after typing stops: going back to the PDF, hiding the panel, leaving the terminal window, quitting or closing the window saves the note. yazi opens notes with `hx` too. `pdfnote` opens the note named after the PDF (its stored title, or else its file name), making it from your template with a link back to the PDF if the vault doesn't have it yet, at its end; `pdfnote Some name` opens or makes the note called that.

`Alt-t` writes a tag for the page you're reading where the terminal's cursor is, such as `(p. 45)`, with the number printed on the page (`(p. xiv)` in the front matter). `Alt-g` moves the PDF to the tag on the cursor's line, or else to the nearest one above it, and Ctrl-click on a tag goes there too. Tags you type yourself work the same: `p. 45`, `p.45`, `pp. 45-47`, and `p. #30` for the 30th page of the file.

## Search

Search ignores case. It runs alongside the pages, so they keep drawing and scrolling while it does: the status bar counts the matches found so far, the matches are highlighted as they turn up, and once it's done it goes to the first match from where you are. All matches are highlighted in yellow, the current one in orange, and the search runs again when the file reloads.

## Configuration

The config is created on first run at `~/.config/viewmer/config.toml` (or under `$XDG_CONFIG_HOME` when that is set), with a line about each setting. It lists the defaults; a setting or key left out keeps its default, so the file only needs what you change, and settings that later versions add work without touching it:

- `[viewer]`: `scroll_step`, `zoom_step` (percent of the page width), `resolution` (pages are drawn at the screen's own resolution times this, again whenever the zoom changes, so they stay sharp when zoomed in; zoomed far in, the part in view is drawn sharp and the pages around it coarser until you get to them, so any zoom fits in the memory kitty keeps for images), `page_gap`, `invert_wheel`, `link_hints` (where a hovered link points) and `remember_position`.
- `[notes]`: `vault` (the notes folder), `folder` (where new notes go, inside the vault, and where the terminal starts), `shell` (empty for the one you log in with), `editor` (what `pdfnote` opens), `template` (`{{title}}` becomes the note's name and `{{source}}` the link to the PDF; without `{{source}}`, the link goes on a `Links:` line or after the title) and `width` (percent of the window).
- `[bindings]`: `"key" = "Action"`, added to the default keys; `"key" = ""` unbinds a default. Key syntax follows [keybinds-rs](https://github.com/rhysd/keybinds-rs/blob/main/doc/binding_syntax.md). The actions are `MoveUp`, `MoveDown`, `MoveLeft`, `MoveRight`, `NextPage`, `PrevPage`, `JumpFirstPage`, `JumpLastPage`, `JumpBack`, `JumpForward`, `SetMark`, `GoToMark`, `CommandLine`, `Search`, `NextMatch`, `PrevMatch`, `ClearSearch`, `ShowOutline`, `HalfPageDown`, `HalfPageUp`, `ZoomIn`, `ZoomOut`, `FitWidth`, `FitPage`, `CenterViewer`, `ToggleRuler`, `ToggleInverse`, `ToggleAlpha`, `ToggleNote`, `FocusPdf`, `WidenNote`, `NarrowNote`, `TagPage`, `GoToTag`, `ShowHelp` and `Quit`. The notes actions bound with Ctrl or Alt also work while the note has the keys.

## Colors

Colors are in `theme.toml` next to the config, written on first run with every color and what it's for. A color can be `"#1e2030"`, `"#fff"`, `"rgb(30, 32, 48)"`, `"hsl(230, 23%, 15%)"`, a palette number like `8`, a name like `"cyan"` or `"bright-black"`, or `"default"` for the terminal's own color.

Panel backgrounds (`contents.background`, `contents.selected`, `help.background`, `notes.background`) and the colors painted on pages (`pages.match`, `pages.current_match`, `pages.ruler`) can be see-through: add two hex digits of opacity, from `00` (clear) to `ff` (solid), as in `"#1e2030cc"`. The page then shows through the panel. A page color without them is mixed in at a strength that keeps the text readable.

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

viewmer is built from source, and built and tested on macOS. It needs Rust, and the Xcode command line tools for building MuPDF:

```sh
cargo install --git https://codeberg.org/deppes/viewmer
```

From a checkout, what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D clippy::all
cargo test --all-features -- --test-threads 1
cargo build --release
cargo install --path .
```

The tests cover the page placement and clipping math, which images a frame places again, reading printed page numbers and the outline from a PDF, folding and filtering the contents panel, the remembered state (moved, rebuilt and broken files), the jump list, marks, finding text lines and placing the ruler, key and prompt handling, search stepping, the help list, reading colors and the theme file, encoding keys, the mouse and pastes for the editor, passing the terminal's images on to kitty, running a program in the notes panel with its scrollback and clipboard copies, naming notes and writing them from a template, finding page tags, reading printed page numbers off the pages, laying the config over the defaults, and the file watcher, which must reload once when a file is replaced by a rename. Drawing in kitty itself is checked by hand.

## License

PolyForm Strict License 1.0.0 (`SPDX-License-Identifier: PolyForm-Strict-1.0.0`). See [LICENSE](LICENSE). The code inherited from MeowPDF stays under its MIT license, kept in [LICENSE-MeowPDF](LICENSE-MeowPDF). Rendering uses MuPDF, which is AGPL-3.0, so a built binary is subject to its terms.

I ask that this code not be used to train AI models. This is a request, not a license term.
