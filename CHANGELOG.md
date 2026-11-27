# Changelog

## 4.0.0 - 2026-11-26

kpv is now tpv: the command, `~/.config/tpv` and `~/.local/state/tpv`. To keep a config and remembered positions, move `~/.config/kpv` and `~/.local/state/kpv` to the new names; a yazi opener needs `kpv` changed to `tpv`.

### Changed

- tpv is described as a viewer for any terminal with the kitty graphics protocol, such as kitty or Rex, and an error names the terminal rather than kitty.

## 3.0.0 - 2026-11-22

viewmer is now kpv: the command, `~/.config/kpv` and `~/.local/state/kpv`. To keep a config and remembered positions, move `~/.config/viewmer` and `~/.local/state/viewmer` to the new names; a yazi opener needs `viewmer` changed to `kpv`.

## 2.0.0 - 2026-11-21

tpv is now the PDF viewer alone.

### Removed

- The notes terminal, with `pdfnote`, page tags and the `[notes]` settings.
- The reading ruler, marks and the jump list.
- Printed page numbers and `:offset`: pages go by their place in the file, and `:45` is the 45th page.
- The crash watcher; closing the window, `kill` and interrupts still quit as `q` does, saving first.
- Transparent pages (`Ctrl-a`).

Configs that bind the removed actions need those keys taken out.

### Smaller

- MuPDF is built without its e-book and Office engine, document export, barcodes, Brotli and spot colors, which a PDF viewer doesn't use: the binary goes from 7.8 MB to 5.4 MB, with pages drawn as before. This comes from `.cargo/config.toml`, so install from a checkout.

## 1.0.0 - 2026-11-19

The first release of tpv, forked from MeowPDF 1.2.2, whose history is in [its repository](https://github.com/monoamine11231/MeowPDF/blob/main/CHANGELOG.md).

### Reading

- Pages render at the screen's own resolution and again on every zoom, so they stay sharp; zoomed far in, the part in view is drawn sharp and the pages around it coarser, within the memory kitty keeps for images.
- A framed window with the document's title and a status bar: mode, page as printed, zoom, section, search.
- Printed page numbers are read off the pages of PDFs that don't say how they're numbered; `:offset` sets them by hand.
- Search runs in the background with highlights, a contents panel with folding and filtering, marks, a jump list, a reading ruler, dark pages, and a key list on `?`.
- Each file reopens where it was left, with its layout, marks and note. Live reload when the file changes, including saves that replace it.

### Notes

- `Ctrl-h` opens a terminal beside the PDF running your shell in the notes folder: helix, yazi and anything else run as in any terminal, with images, scrollback and the clipboard. `Ctrl-l` goes back; `Ctrl-h` from inside hides it.
- `pdfnote` opens the note named after the PDF, made from your template with a link back to it; `hx` there saves when focus leaves the panel.
- `Alt-t` tags the page you're reading in the note; `Alt-g` and Ctrl-click go to a tagged page.

### Settings

- `config.toml` and `theme.toml` in `~/.config/tpv`: a config only needs what it changes, colors take any common notation and panels can be see-through.

### Robustness

- A crash watcher gives the terminal back, logs the crash and starts the viewer again where it was. Closing the window, `kill` and interrupts quit as `q` does, saving first; state files are written whole and synced before they replace the old ones.
- Quitting frees the images tpv gave kitty.
