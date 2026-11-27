//! The links on a screen: an address a program printed, a link it marked
//! with OSC 8, the escape code of a link in a terminal, and a place in a
//! file like `src/main.rs:3:7`. A long one the terminal wrapped onto the
//! next rows is found whole.

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Term, cell::Flags},
};
use hilen::system::is_safe_link;
use linkify::{LinkFinder, LinkKind};

/// The rows above and below a cell that are read for its link. A line the
/// terminal wrapped more often than that is cut there.
const MAX_WRAPPED_ROWS: i32 = 100;

/// A link on the screen, with its first and its last cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url:   String,
    pub start: Point,
    pub end:   Point,
    /// A place in a file, like `src/main.rs:3:7`, for the owner to open,
    /// not an address for the browser.
    pub place: bool,
}

impl Link {
    pub fn contains(&self, point: Point) -> bool {
        self.start <= point && point <= self.end
    }
}

/// The link at a cell of the grid. None when the cell holds no link, or a
/// link that must not open, like `javascript:`, see `is_safe_link`.
pub fn link_at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    if point.line < term.topmost_line()
        || point.line > term.bottommost_line()
        || point.column > term.last_column()
    {
        return None;
    }
    marked_link(term, point)
        .or_else(|| printed_link(term, point))
        .filter(|link| link.place || is_safe_link(&link.url))
}

/// A link a program marked with OSC 8: the cells in a row that hold the
/// same link.
fn marked_link<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let hyperlink = grid[point].hyperlink()?;
    let same = |at: Point| grid[at].hyperlink().as_ref() == Some(&hyperlink);
    let mut start = point;
    while let Some(before) = previous(term, start)
        && same(before)
    {
        start = before;
    }
    let mut end = point;
    while let Some(after) = next(term, end)
        && same(after)
    {
        end = after;
    }
    Some(Link {
        url: hyperlink.uri().to_string(),
        start,
        end,
        place: false,
    })
}

/// An address a program printed, read from the whole line of the cell,
/// with the rows the terminal wrapped that line onto.
fn printed_link<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let last = term.last_column();
    let wraps = |line: i32| grid[Line(line)][last].flags.contains(Flags::WRAPLINE);
    let row = point.line.0;
    let mut first = row;
    while first > term.topmost_line().0 && row - first < MAX_WRAPPED_ROWS && wraps(first - 1) {
        first -= 1;
    }
    let mut end_row = row;
    while end_row < term.bottommost_line().0 && end_row - row < MAX_WRAPPED_ROWS && wraps(end_row) {
        end_row += 1;
    }

    // The text of the line, and where in it the text of each cell starts.
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut pointed = 0;
    for line in first..=end_row {
        for column in 0..=last.0 {
            let at = Point::new(Line(line), Column(column));
            let cell = &grid[at];
            // The second half of a wide char holds nothing of its own, a
            // point on it means the char.
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            if at <= point {
                pointed = text.len();
            }
            cells.push((text.len(), at));
            text.push(cell.c);
            text.extend(cell.zerowidth().unwrap_or_default());
        }
    }

    let (from, to, place) = LinkFinder::new()
        .kinds(&[LinkKind::Url])
        .links(&text)
        .find(|link| link.start() <= pointed && pointed < link.end())
        .map(|link| (link.start(), link.end(), false))
        .or_else(|| place_around(&text, pointed).map(|(from, to)| (from, to, true)))?;
    let start = cells.iter().find(|(byte, _)| *byte >= from)?.1;
    let end = cells.iter().rev().find(|(byte, _)| *byte < to)?.1;
    Some(Link {
        url: text[from..to].to_string(),
        start,
        end,
        place,
    })
}

/// The place in a file around byte `at` of `text`, as its range of bytes.
/// The colon a panic prints after its place is not part of it.
fn place_around(text: &str, at: usize) -> Option<(usize, usize)> {
    let stop = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | '`' | ',' | ';'
            )
    };
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(_, c)| stop(*c))
        .map_or(0, |(byte, c)| byte + c.len_utf8());
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| stop(*c))
        .map_or(text.len(), |(byte, _)| at + byte);
    let place = text[start..end].trim_end_matches([':', '.']);
    let end = start + place.len();
    (at < end && is_place(place)).then_some((start, end))
}

/// A path with a dot or a slash in it, then `:` and a line number, and
/// maybe `:` and a column, like `src/main.rs:3:7` or `/a/build.rs:12`.
fn is_place(text: &str) -> bool {
    let number = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let Some((rest, last)) = text.rsplit_once(':') else {
        return false;
    };
    if !number(last) {
        return false;
    }
    let path = match rest.rsplit_once(':') {
        Some((path, line)) if number(line) => path,
        _ => rest,
    };
    !path.is_empty() && !path.contains("://") && (path.contains('/') || path.contains('.'))
}

/// The cell before `point`, the last one of the row above for the first
/// cell of a row.
fn previous<T>(term: &Term<T>, point: Point) -> Option<Point> {
    if point.column.0 > 0 {
        return Some(Point::new(point.line, Column(point.column.0 - 1)));
    }
    (point.line > term.topmost_line()).then(|| Point::new(Line(point.line.0 - 1), term.last_column()))
}

/// The cell after `point`, the first one of the row below for the last
/// cell of a row.
fn next<T>(term: &Term<T>, point: Point) -> Option<Point> {
    if point.column < term.last_column() {
        return Some(Point::new(point.line, Column(point.column.0 + 1)));
    }
    (point.line < term.bottommost_line()).then(|| Point::new(Line(point.line.0 + 1), Column(0)))
}

#[cfg(test)]
mod tests {
    use crate::screen::{Cells, Screen};

    fn screen(columns: usize) -> Screen {
        Screen::new(Cells { columns, lines: 6 }, (8, 16))
    }

    fn no_color(_: usize) -> (u8, u8, u8) {
        (0, 0, 0)
    }

    fn url_at(screen: &Screen, column: usize, row: usize) -> Option<String> {
        screen.link_at(column, row).map(|link| link.url)
    }

    /// The line `infisical login` prints, the quote and the dot after the
    /// address are not part of it.
    #[test]
    fn a_printed_address_is_a_link_and_the_text_around_it_is_not() {
        let mut screen = screen(80);
        screen.feed(
            b"open 'https://infisical.example.net/login?callback_port=40131'.\x1b[?25l",
            no_color,
        );
        let url = Some("https://infisical.example.net/login?callback_port=40131".to_string());
        assert_eq!(url_at(&screen, 6, 0), url);
        assert_eq!(url_at(&screen, 60, 0), url);
        assert_eq!(url_at(&screen, 5, 0), None);
        assert_eq!(url_at(&screen, 61, 0), None);
        assert_eq!(url_at(&screen, 62, 0), None);
        assert_eq!(url_at(&screen, 70, 0), None);
        assert_eq!(url_at(&screen, 10, 1), None);
    }

    #[test]
    fn an_address_the_terminal_wrapped_is_found_whole_from_every_row() {
        let mut screen = screen(20);
        screen.feed(b"see https://example.com/a/very/long/path ok\x1b[?25l", no_color);
        let url = Some("https://example.com/a/very/long/path".to_string());
        assert_eq!(url_at(&screen, 4, 0), url);
        assert_eq!(url_at(&screen, 3, 1), url);
        assert_eq!(url_at(&screen, 0, 2), None);
    }

    #[test]
    fn a_wide_char_before_an_address_does_not_move_it() {
        let mut screen = screen(30);
        screen.feed("漢 https://example.com/x\x1b[?25l".as_bytes(), no_color);
        assert_eq!(url_at(&screen, 3, 0).as_deref(), Some("https://example.com/x"));
        assert_eq!(url_at(&screen, 1, 0), None);
    }

    // The failure this prevents: a place of the compiler that is plain
    // text, so a click on it goes nowhere.
    #[test]
    fn a_printed_place_is_a_link_for_the_owner() {
        let mut screen = screen(60);
        screen.feed(
            b" --> src/number.rs:3:7\r\npanicked at src/main.rs:12:9:\r\nat 12:30:45 ok\x1b[?25l",
            no_color,
        );
        let place = screen.link_at(6, 0).unwrap();
        assert_eq!((place.url.as_str(), place.place), ("src/number.rs:3:7", true));
        assert_eq!(url_at(&screen, 21, 0).as_deref(), Some("src/number.rs:3:7"));
        assert_eq!(url_at(&screen, 2, 0), None);
        assert_eq!(url_at(&screen, 14, 1).as_deref(), Some("src/main.rs:12:9"));
        assert_eq!(url_at(&screen, 28, 1), None);
        // A time is no place, it has no path.
        assert_eq!(url_at(&screen, 5, 2), None);
    }

    #[test]
    fn a_link_a_program_marked_opens_its_own_address() {
        let mut screen = screen(30);
        screen.feed(
            b"read \x1b]8;;https://example.com/docs\x1b\\the docs\x1b]8;;\x1b\\ now\x1b[?25l",
            no_color,
        );
        assert_eq!(url_at(&screen, 5, 0).as_deref(), Some("https://example.com/docs"));
        assert_eq!(
            url_at(&screen, 12, 0).as_deref(),
            Some("https://example.com/docs")
        );
        assert_eq!(url_at(&screen, 14, 0), None);
    }

    #[test]
    fn a_link_that_could_run_code_or_read_a_file_is_no_link() {
        let mut screen = screen(40);
        screen.feed(
            b"\x1b]8;;javascript:alert(1)\x1b\\click\x1b]8;;\x1b\\\r\n",
            no_color,
        );
        screen.feed(b"file:///etc/passwd\x1b[?25l", no_color);
        assert_eq!(url_at(&screen, 1, 0), None);
        assert_eq!(url_at(&screen, 8, 1), None);
    }

    #[test]
    fn the_link_under_the_pointer_has_a_line_under_it() {
        let mut screen = screen(40);
        screen.feed(b"go https://example.com/a now\x1b[?25l", no_color);
        let underlined = |screen: &Screen| -> Vec<String> {
            let row = &screen.rows()[0];
            row.spans
                .iter()
                .filter(|span| span.look.lines.under)
                .map(|span| row.text[span.range.clone()].to_string())
                .collect()
        };
        assert_eq!(underlined(&screen), Vec::<String>::new());
        assert!(!screen.hover(Some((1, 0))));
        assert!(!screen.link_hovered());
        assert!(screen.hover(Some((5, 0))));
        assert!(screen.link_hovered());
        assert_eq!(underlined(&screen), ["https://example.com/a"]);
        assert!(!screen.hover(Some((8, 0))));
        assert!(screen.hover(None));
        assert!(!screen.link_hovered());
        assert_eq!(underlined(&screen), Vec::<String>::new());
    }
}
