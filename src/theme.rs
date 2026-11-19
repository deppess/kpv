/* Colors, read from theme.toml next to the config. Every slot has a default, so a
 * missing or broken entry never stops the viewer: it falls back and says why. */

use std::{path::Path, sync::OnceLock};

use crossterm::style::Color;
use toml::{Table, Value};

/* One color from the theme */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /* The terminal's own color */
    Default,
    /* A palette entry: the 16 named colors are 0 to 15 */
    Ansi(u8),
    /* `alpha` is `None` when the color didn't give one */
    Rgb {
        r: u8,
        g: u8,
        b: u8,
        alpha: Option<u8>,
    },
}

impl Paint {
    /* The color for terminal cells, without its opacity */
    pub fn color(self) -> Color {
        match self {
            Paint::Default => Color::Reset,
            Paint::Ansi(n) => Color::AnsiValue(n),
            Paint::Rgb { r, g, b, .. } => Color::Rgb { r, g, b },
        }
    }

    /* Red, green and blue, for painting onto pages; palette colors are taken as the
     * standard xterm palette has them. `None` for the terminal's own color. */
    pub fn rgb(self) -> Option<[u8; 3]> {
        match self {
            Paint::Default => None,
            Paint::Ansi(n) => Some(xterm(n)),
            Paint::Rgb { r, g, b, .. } => Some([r, g, b]),
        }
    }

    pub fn alpha(self) -> Option<u8> {
        match self {
            Paint::Rgb { alpha, .. } => alpha,
            _ => None,
        }
    }

    /* A background that lets the page show through it */
    pub fn see_through(self) -> bool {
        self.alpha().is_some_and(|a| a < 255)
    }

    fn opaque(self) -> Self {
        match self {
            Paint::Rgb { r, g, b, .. } => Paint::Rgb {
                r,
                g,
                b,
                alpha: None,
            },
            other => other,
        }
    }
}

const NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/* Reads one color: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`,
 * `hsl(h, s%, l%)`, a palette number 0-255, one of the 16 names, or `default` */
pub fn parse_color(text: &str) -> Result<Paint, String> {
    let text = text.trim().to_ascii_lowercase();
    let bad = || format!("not a color: \"{}\"", text);

    if matches!(text.as_str(), "default" | "none") {
        return Ok(Paint::Default);
    }
    if let Some(hex) = text.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).unwrap() * 17;
        let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        return match hex.len() {
            3 | 4 => Ok(Paint::Rgb {
                r: digit(0),
                g: digit(1),
                b: digit(2),
                alpha: (hex.len() == 4).then(|| digit(3)),
            }),
            6 | 8 => Ok(Paint::Rgb {
                r: pair(0),
                g: pair(2),
                b: pair(4),
                alpha: (hex.len() == 8).then(|| pair(6)),
            }),
            _ => Err(bad()),
        };
    }
    if let Some(args) = function(&text, "rgb") {
        let [r, g, b] = numbers(&args, bad)?;
        let byte = |v: f64| (0.0..=255.0).contains(&v).then_some(v.round() as u8);
        return match (byte(r), byte(g), byte(b)) {
            (Some(r), Some(g), Some(b)) => Ok(Paint::Rgb {
                r,
                g,
                b,
                alpha: None,
            }),
            _ => Err(format!("rgb() takes numbers from 0 to 255: \"{}\"", text)),
        };
    }
    if let Some(args) = function(&text, "hsl") {
        let [h, s, l] = numbers(&args, bad)?;
        if !(0.0..=100.0).contains(&s) || !(0.0..=100.0).contains(&l) {
            return Err(format!(
                "hsl() takes percentages from 0 to 100: \"{}\"",
                text
            ));
        }
        let [r, g, b] = hsl(h, s / 100.0, l / 100.0);
        return Ok(Paint::Rgb {
            r,
            g,
            b,
            alpha: None,
        });
    }
    if function(&text, "rgba").is_some() || function(&text, "hsla").is_some() {
        return Err(format!(
            "for opacity, write the color as #rrggbbaa: \"{}\"",
            text
        ));
    }
    if let Ok(n) = text.parse::<u16>() {
        return u8::try_from(n)
            .map(Paint::Ansi)
            .map_err(|_| format!("palette colors go from 0 to 255: {}", n));
    }
    let name = text.replace(['_', ' '], "-");
    let name = match name.as_str() {
        "grey" | "gray" => "bright-black",
        other => other,
    };
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| Paint::Ansi(i as u8))
        .ok_or_else(bad)
}

/* The arguments of `name(...)`, if `text` is that */
fn function(text: &str, name: &str) -> Option<String> {
    let rest = text.strip_prefix(name)?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.to_owned())
}

/* Three numbers separated by commas or spaces; `%` signs are allowed */
fn numbers(args: &str, bad: impl Fn() -> String) -> Result<[f64; 3], String> {
    let parts: Vec<f64> = args
        .split([',', ' '])
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.trim_end_matches('%')
                .trim_end_matches("deg")
                .parse::<f64>()
        })
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    parts.try_into().map_err(|_| bad())
}

fn hsl(h: f64, s: f64, l: f64) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

/* The standard xterm palette */
fn xterm(n: u8) -> [u8; 3] {
    const BASE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 0, 0],
        [0, 205, 0],
        [205, 205, 0],
        [0, 0, 238],
        [205, 0, 205],
        [0, 205, 205],
        [229, 229, 229],
        [127, 127, 127],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [92, 92, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    match n {
        0..=15 => BASE[n as usize],
        16..=231 => {
            let i = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            [level(i / 36), level(i / 6 % 6), level(i % 6)]
        }
        _ => {
            let v = 8 + 10 * (n - 232);
            [v, v, v]
        }
    }
}

/* Every color slot. `Slot as usize` indexes `SPECS` and `Theme::paints`. */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    FrameBorder,
    FrameBorderActive,
    FrameTitle,
    StatusText,
    StatusSeparator,
    StatusNormal,
    StatusCommand,
    StatusSearch,
    StatusContents,
    StatusHelp,
    StatusNotes,
    StatusMatches,
    StatusMessage,
    StatusLink,
    StatusSection,
    ContentsBackground,
    ContentsBorder,
    ContentsTitle,
    ContentsText,
    ContentsMuted,
    ContentsSelected,
    ContentsSelectedText,
    ContentsCurrent,
    HelpBackground,
    HelpBorder,
    HelpHeading,
    HelpKeys,
    HelpText,
    HelpMuted,
    NotesBackground,
    NotesText,
    LayerBackground,
    LayerBorder,
    LayerTitle,
    LayerKeys,
    LayerText,
    PagesMatch,
    PagesCurrentMatch,
    PagesRuler,
    PagesDarkBackground,
    PagesDarkText,
}

/* Where a slot lives in theme.toml, its default, and whether it may be see-through */
pub struct Spec {
    pub slot: Slot,
    pub section: &'static str,
    pub key: &'static str,
    pub default: &'static str,
    pub see_through: bool,
    pub about: &'static str,
}

const fn spec(
    slot: Slot,
    section: &'static str,
    key: &'static str,
    default: &'static str,
    see_through: bool,
    about: &'static str,
) -> Spec {
    Spec {
        slot,
        section,
        key,
        default,
        see_through,
        about,
    }
}

pub const SPECS: [Spec; 41] = [
    spec(
        Slot::FrameBorder,
        "frame",
        "border",
        "bright-black",
        false,
        "The rounded border",
    ),
    spec(
        Slot::FrameBorderActive,
        "frame",
        "border_active",
        "bright-cyan",
        false,
        "The border while the PDF has the keys beside the notes terminal",
    ),
    spec(
        Slot::FrameTitle,
        "frame",
        "title",
        "default",
        false,
        "The file name in the top edge",
    ),
    spec(
        Slot::StatusText,
        "status",
        "text",
        "default",
        false,
        "Page and zoom",
    ),
    spec(
        Slot::StatusSeparator,
        "status",
        "separator",
        "bright-black",
        false,
        "The │ between pieces",
    ),
    spec(
        Slot::StatusNormal,
        "status",
        "normal",
        "bright-cyan",
        false,
        "NOR",
    ),
    spec(
        Slot::StatusCommand,
        "status",
        "command",
        "bright-cyan",
        false,
        "The : prompt",
    ),
    spec(
        Slot::StatusSearch,
        "status",
        "search",
        "bright-cyan",
        false,
        "The / prompt",
    ),
    spec(
        Slot::StatusContents,
        "status",
        "contents",
        "bright-cyan",
        false,
        "TOC",
    ),
    spec(
        Slot::StatusHelp,
        "status",
        "help",
        "bright-cyan",
        false,
        "HELP",
    ),
    spec(
        Slot::StatusNotes,
        "status",
        "notes",
        "bright-cyan",
        false,
        "NOTE",
    ),
    spec(
        Slot::StatusMatches,
        "status",
        "matches",
        "bright-yellow",
        false,
        "The search and its match number",
    ),
    spec(
        Slot::StatusMessage,
        "status",
        "message",
        "default",
        false,
        "Short messages",
    ),
    spec(
        Slot::StatusLink,
        "status",
        "link",
        "default",
        false,
        "Where a hovered link points",
    ),
    spec(
        Slot::StatusSection,
        "status",
        "section",
        "default",
        false,
        "The section you're reading",
    ),
    spec(
        Slot::ContentsBackground,
        "contents",
        "background",
        "black",
        true,
        "Behind the panel",
    ),
    spec(
        Slot::ContentsBorder,
        "contents",
        "border",
        "bright-cyan",
        false,
        "Its border",
    ),
    spec(
        Slot::ContentsTitle,
        "contents",
        "title",
        "bright-cyan",
        false,
        "\"Contents\" in the top edge",
    ),
    spec(
        Slot::ContentsText,
        "contents",
        "text",
        "bright-white",
        false,
        "Entry titles",
    ),
    spec(
        Slot::ContentsMuted,
        "contents",
        "muted",
        "bright-black",
        false,
        "Page numbers, fold markers and hints",
    ),
    spec(
        Slot::ContentsSelected,
        "contents",
        "selected",
        "bright-black",
        true,
        "Behind the selected entry",
    ),
    spec(
        Slot::ContentsSelectedText,
        "contents",
        "selected_text",
        "bright-white",
        false,
        "The selected entry's page number",
    ),
    spec(
        Slot::ContentsCurrent,
        "contents",
        "current",
        "bright-cyan",
        false,
        "The section you're reading",
    ),
    spec(
        Slot::HelpBackground,
        "help",
        "background",
        "black",
        true,
        "Behind the key list",
    ),
    spec(
        Slot::HelpBorder,
        "help",
        "border",
        "bright-cyan",
        false,
        "Its border",
    ),
    spec(
        Slot::HelpHeading,
        "help",
        "heading",
        "bright-cyan",
        false,
        "The names of the groups",
    ),
    spec(Slot::HelpKeys, "help", "keys", "default", false, "The keys"),
    spec(
        Slot::HelpText,
        "help",
        "text",
        "default",
        false,
        "What they do",
    ),
    spec(
        Slot::HelpMuted,
        "help",
        "muted",
        "bright-black",
        false,
        "The hint in the bottom edge",
    ),
    spec(
        Slot::NotesBackground,
        "notes",
        "background",
        "default",
        true,
        "Behind the note",
    ),
    spec(
        Slot::NotesText,
        "notes",
        "text",
        "default",
        false,
        "The editor's own text color",
    ),
    spec(
        Slot::LayerBackground,
        "layer",
        "background",
        "black",
        true,
        "Behind a layer's keys",
    ),
    spec(
        Slot::LayerBorder,
        "layer",
        "border",
        "bright-cyan",
        false,
        "Its border",
    ),
    spec(
        Slot::LayerTitle,
        "layer",
        "title",
        "bright-cyan",
        false,
        "The layer's name",
    ),
    spec(
        Slot::LayerKeys,
        "layer",
        "keys",
        "bright-yellow",
        false,
        "The keys",
    ),
    spec(
        Slot::LayerText,
        "layer",
        "text",
        "default",
        false,
        "What they do",
    ),
    spec(
        Slot::PagesMatch,
        "pages",
        "match",
        "#ffd600",
        true,
        "Search matches",
    ),
    spec(
        Slot::PagesCurrentMatch,
        "pages",
        "current_match",
        "#ff7800",
        true,
        "The match you're on",
    ),
    spec(
        Slot::PagesRuler,
        "pages",
        "ruler",
        "#5aa0ff",
        true,
        "The reading ruler",
    ),
    spec(
        Slot::PagesDarkBackground,
        "pages",
        "dark_background",
        "#1e1e24",
        false,
        "Paper on dark pages",
    ),
    spec(
        Slot::PagesDarkText,
        "pages",
        "dark_text",
        "#dcdce0",
        false,
        "Text on dark pages",
    ),
];

const HEADER: &str = "\
# viewmer colors. A color can be written as
#   \"#1e2030\" or \"#fff\"          hex
#   \"rgb(30, 32, 48)\"            red, green and blue from 0 to 255
#   \"hsl(230, 23%, 15%)\"         hue, saturation and lightness
#   8                            a palette number from 0 to 255
#   \"cyan\", \"bright-black\"       one of the 16 terminal colors
#   \"default\"                    the terminal's own color
#
# Panel backgrounds and the colors painted on pages can also be see-through:
# add two hex digits of opacity, from 00 (clear) to ff (solid), as in
# \"#1e2030cc\" or \"#fffc\". On pages, a color without them is mixed in at a
# strength that keeps the text readable. Palette colors painted on pages use
# the standard xterm palette.
";

/* theme.toml as first written: every slot with its default */
pub fn default_file() -> String {
    let mut out = String::from(HEADER);
    let mut section = "";
    for spec in &SPECS {
        if spec.section != section {
            section = spec.section;
            out.push_str(&format!("\n[{}]\n", section));
        }
        let line = format!("{} = \"{}\"", spec.key, spec.default);
        out.push_str(&format!("{:<34}# {}\n", line, spec.about));
    }
    out
}

pub struct Theme {
    paints: Vec<Paint>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            paints: SPECS
                .iter()
                .map(|s| parse_color(s.default).expect("default colors parse"))
                .collect(),
        }
    }
}

impl Theme {
    pub fn get(&self, slot: Slot) -> Paint {
        self.paints[slot as usize]
    }

    /* Reads theme.toml's contents. Entries that are wrong keep their default and are
     * listed in the second value, one line each. */
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut theme = Theme::default();
        let mut problems = Vec::new();
        let table = match text.parse::<Table>() {
            Ok(table) => table,
            Err(e) => {
                let first = e.to_string().lines().next().unwrap_or_default().to_owned();
                return (theme, vec![format!("not readable: {}", first)]);
            }
        };

        for (section, values) in &table {
            let Some(values) = values.as_table() else {
                problems.push(format!("{}: not a [section]", section));
                continue;
            };
            for (key, value) in values {
                let name = format!("{}.{}", section, key);
                let Some(spec) =
                    SPECS.iter().find(|s| s.section == section && s.key == key)
                else {
                    problems.push(format!("{}: no such color", name));
                    continue;
                };
                let paint = match value {
                    Value::String(text) => parse_color(text),
                    Value::Integer(n) => parse_color(&n.to_string()),
                    _ => Err("write the color in quotes".to_owned()),
                };
                match paint {
                    Ok(paint) if paint.alpha().is_some() && !spec.see_through => {
                        problems.push(format!(
                            "{}: only backgrounds and page colors can be see-through",
                            name
                        ));
                        theme.paints[spec.slot as usize] = paint.opaque();
                    }
                    Ok(paint) => theme.paints[spec.slot as usize] = paint,
                    Err(e) => problems.push(format!("{}: {}", name, e)),
                }
            }
        }
        (theme, problems)
    }

    /* Reads `path`, writing the defaults there first if it doesn't exist */
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        if !path.exists() {
            let _ = std::fs::write(path, default_file());
            return (Theme::default(), Vec::new());
        }
        match std::fs::read_to_string(path) {
            Ok(text) => Theme::parse(&text),
            Err(e) => (Theme::default(), vec![format!("not readable: {}", e)]),
        }
    }
}

static THEME: OnceLock<Theme> = OnceLock::new();

/* Sets the theme for the rest of the run; only the first call counts */
pub fn set_theme(theme: Theme) {
    let _ = THEME.set(theme);
}

/* The theme in use, or the defaults before one is set (as in tests) */
pub fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

pub fn paint(slot: Slot) -> Paint {
    theme().get(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8, alpha: Option<u8>) -> Paint {
        Paint::Rgb { r, g, b, alpha }
    }

    #[test]
    fn hex_colors_with_and_without_opacity() {
        assert_eq!(parse_color("#1e2030"), Ok(rgb(0x1e, 0x20, 0x30, None)));
        assert_eq!(
            parse_color("#1E2030CC"),
            Ok(rgb(0x1e, 0x20, 0x30, Some(0xcc)))
        );
        assert_eq!(parse_color("#fff"), Ok(rgb(255, 255, 255, None)));
        assert_eq!(parse_color("#fffc"), Ok(rgb(255, 255, 255, Some(0xcc))));
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#ggg").is_err());
    }

    #[test]
    fn rgb_and_hsl() {
        assert_eq!(parse_color("rgb(30, 32, 48)"), Ok(rgb(30, 32, 48, None)));
        assert_eq!(parse_color("rgb(30 32 48)"), Ok(rgb(30, 32, 48, None)));
        assert!(parse_color("rgb(300, 0, 0)").is_err());
        assert_eq!(parse_color("hsl(0, 100%, 50%)"), Ok(rgb(255, 0, 0, None)));
        assert_eq!(parse_color("hsl(120, 100%, 25%)"), Ok(rgb(0, 128, 0, None)));
        assert_eq!(
            parse_color("hsl(240deg, 100%, 50%)"),
            Ok(rgb(0, 0, 255, None))
        );
        let err = parse_color("rgba(1, 2, 3, 0.5)").unwrap_err();
        assert!(err.contains("#rrggbbaa"), "{}", err);
    }

    #[test]
    fn names_numbers_and_default() {
        assert_eq!(parse_color("cyan"), Ok(Paint::Ansi(6)));
        assert_eq!(parse_color("Bright Cyan"), Ok(Paint::Ansi(14)));
        assert_eq!(parse_color("bright_black"), Ok(Paint::Ansi(8)));
        assert_eq!(parse_color("grey"), Ok(Paint::Ansi(8)));
        assert_eq!(parse_color("200"), Ok(Paint::Ansi(200)));
        assert!(parse_color("256").is_err());
        assert_eq!(parse_color("default"), Ok(Paint::Default));
        assert!(parse_color("blu").is_err());
    }

    #[test]
    fn palette_colors_have_xterm_values_for_pages() {
        assert_eq!(Paint::Ansi(9).rgb(), Some([255, 0, 0]));
        assert_eq!(Paint::Ansi(196).rgb(), Some([255, 0, 0]));
        assert_eq!(Paint::Ansi(232).rgb(), Some([8, 8, 8]));
        assert_eq!(Paint::Default.rgb(), None);
    }

    #[test]
    fn slots_and_specs_line_up() {
        for (i, spec) in SPECS.iter().enumerate() {
            assert_eq!(spec.slot as usize, i, "{}.{}", spec.section, spec.key);
            assert!(parse_color(spec.default).is_ok());
        }
    }

    #[test]
    fn the_written_defaults_read_back_without_problems() {
        let (theme, problems) = Theme::parse(&default_file());
        assert!(problems.is_empty(), "{:?}", problems);
        assert_eq!(theme.paints, Theme::default().paints);
    }

    #[test]
    fn mistakes_keep_the_default_and_say_why() {
        let (theme, problems) = Theme::parse(
            "[frame]\nborder = \"blu\"\ntitle = \"#ff000080\"\n[contents]\nbackground = \"#10101080\"\nfoo = \"red\"\n",
        );
        assert_eq!(theme.get(Slot::FrameBorder), Paint::Ansi(8));
        assert_eq!(theme.get(Slot::FrameTitle), rgb(255, 0, 0, None));
        assert!(theme.get(Slot::ContentsBackground).see_through());
        assert_eq!(problems.len(), 3, "{:?}", problems);
        let said = |text: &str| problems.iter().any(|p| p.contains(text));
        assert!(said("frame.border: not a color"));
        assert!(said(
            "frame.title: only backgrounds and page colors can be see-through"
        ));
        assert!(said("contents.foo: no such color"));
    }
}
