//! The screen of a terminal: what a shell printed, as rows of cells. The
//! bytes go through the emulator of Alacritty, the same code that draws
//! a shell there, so a program sees the terminal it knows. Nothing here
//! draws, the page turns the rows into labels.

use std::{cell::RefCell, mem::take, ops::Range, rc::Rc};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Config, Term, TermMode, cell::Flags},
    vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb},
};

use crate::link::{self, Link};

/// The color of a cell before the theme gives it its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    /// The text color of the terminal.
    Foreground,
    /// The color behind the text of the terminal.
    Background,
    /// One of the 256 colors a program picks by number.
    Indexed(u8),
    /// A color a program gave by its parts.
    Rgb(u8, u8, u8),
}

/// Cells in a row that look the same, as a byte range of the row text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub look:  Look,
}

/// How a cell looks, what the cells of 1 span share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub text:  Paint,
    pub back:  Paint,
    pub bold:  bool,
    pub dim:   bool,
    pub lines: Lines,
}

/// The lines drawn at a cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lines {
    /// Under the text.
    pub under:   bool,
    /// Through the text.
    pub through: bool,
}

/// One row of the screen. The text ends at its last cell that shows
/// something, an empty row has no text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub text:  String,
    pub spans: Vec<Span>,
}

/// What a program asked for while its output was read.
#[derive(Default)]
pub struct Asked {
    /// Bytes to send back to the shell, the answers to its questions.
    pub replies: Vec<u8>,
    /// The new title of the terminal.
    pub title:   Option<String>,
    /// A text a program put into the clipboard.
    pub copied:  Option<String>,
}

/// What the emulator hands out while it reads, kept until the read is
/// over: the emulator cannot be asked anything in the middle of it.
#[derive(Default)]
struct Heard {
    events: Vec<Event>,
}

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Heard>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().events.push(event);
    }
}

/// The size of a screen in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells {
    pub columns: usize,
    pub lines:   usize,
}

impl Dimensions for Cells {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// How a kind of selection grows from the press that started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// From cell to cell.
    Cells,
    /// By words, a double click.
    Words,
}

pub struct Screen {
    term:        Term<Listener>,
    parser:      Processor,
    heard:       Listener,
    size:        Cells,
    /// The size of 1 cell in pixels, a program may ask for it.
    cell_pixels: (u16, u16),
    /// The cell of the view the pointer is over, None off the cells.
    hover:       Option<(usize, usize)>,
}

/// The lines kept above the screen, to scroll back to.
const SCROLLBACK: usize = 10_000;

impl Screen {
    pub fn new(size: Cells, cell_pixels: (u16, u16)) -> Self {
        let heard = Listener::default();
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, heard.clone()),
            parser: Processor::new(),
            heard,
            size,
            cell_pixels,
            hover: None,
        }
    }

    pub fn size(&self) -> Cells {
        self.size
    }

    /// Reads what the shell printed. `color` is the value of a color by
    /// its number, as the theme has it now, for a program that asks.
    pub fn feed(&mut self, bytes: &[u8], color: impl Fn(usize) -> (u8, u8, u8)) -> Asked {
        self.parser.advance(&mut self.term, bytes);
        let events = take(&mut self.heard.0.borrow_mut().events);
        let mut asked = Asked::default();
        for event in events {
            match event {
                Event::PtyWrite(text) => asked.replies.extend(text.into_bytes()),
                Event::Title(title) => asked.title = Some(title),
                Event::ResetTitle => asked.title = Some(String::new()),
                Event::ClipboardStore(_, text) => asked.copied = Some(text),
                Event::ColorRequest(index, format) => {
                    let set = self.term.colors()[index];
                    let (r, g, b) = set.map_or_else(|| color(index), |rgb| (rgb.r, rgb.g, rgb.b));
                    asked.replies.extend(format(Rgb { r, g, b }).into_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines:   u16::try_from(self.size.lines).unwrap_or(u16::MAX),
                        num_cols:    u16::try_from(self.size.columns).unwrap_or(u16::MAX),
                        cell_width:  self.cell_pixels.0,
                        cell_height: self.cell_pixels.1,
                    };
                    asked.replies.extend(format(size).into_bytes());
                }
                // A program that reads the clipboard gets nothing: what
                // runs on another machine must not read it unasked.
                Event::ClipboardLoad(..)
                | Event::MouseCursorDirty
                | Event::CursorBlinkingChange
                | Event::Wakeup
                | Event::Bell
                | Event::Exit
                | Event::ChildExit(_) => {}
            }
        }
        asked
    }

    pub fn resize(&mut self, size: Cells, cell_pixels: (u16, u16)) {
        self.size = size;
        self.cell_pixels = cell_pixels;
        self.term.resize(size);
    }

    /// Moves the view over the scrollback, up for a positive number.
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Back to the newest lines, where the shell prints.
    pub fn scroll_to_end(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// A program draws the whole screen itself, like an editor. It has
    /// no scrollback, the wheel then moves inside the program.
    pub fn full_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// The wheel is to be sent to a full screen program as arrow keys.
    pub fn wheel_as_arrows(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
    }

    /// The bytes of 1 wheel step at a cell, for a program that asked for
    /// mouse reports, like codex. None when it did not ask, or when the
    /// cell is too far for the old code that has 1 byte per number.
    pub fn wheel_report(&self, up: bool, column: usize, row: usize) -> Option<Vec<u8>> {
        let mode = self.term.mode();
        if !mode.intersects(TermMode::MOUSE_MODE) {
            return None;
        }
        let button: u8 = if up { 64 } else { 65 };
        if mode.contains(TermMode::SGR_MOUSE) {
            return Some(format!("\x1b[<{button};{};{}M", column + 1, row + 1).into_bytes());
        }
        let byte = |cell: usize| u8::try_from(cell + 33).ok();
        Some(vec![0x1b, b'[', b'M', 32 + button, byte(column)?, byte(row)?])
    }

    /// The arrows and Home and End have their other codes now, a
    /// program like an editor asked for them.
    pub fn application_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// A paste is to be wrapped in its marks, so the shell takes a new
    /// line in it as text.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// The cell of the grid at a column and a row of the view.
    fn point(&self, column: usize, row: usize) -> Point {
        let offset = i32::try_from(self.term.grid().display_offset()).unwrap_or(i32::MAX);
        let row = i32::try_from(row).unwrap_or(i32::MAX);
        let column = column.min(self.size.columns.saturating_sub(1));
        Point::new(Line(row.saturating_sub(offset)), Column(column))
    }

    /// A press starts a selection at this cell of the view.
    pub fn select_from(&mut self, column: usize, row: usize, pick: Pick) {
        let kind = match pick {
            Pick::Cells => SelectionType::Simple,
            Pick::Words => SelectionType::Semantic,
        };
        self.term.selection = Some(Selection::new(kind, self.point(column, row), Side::Left));
    }

    /// The pointer moved to this cell with the button held.
    pub fn select_to(&mut self, column: usize, row: usize) {
        let point = self.point(column, row);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, Side::Right);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// The selected text, none with no selection or an empty one.
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    /// The address of the link at a cell of the view.
    pub fn link_at(&self, column: usize, row: usize) -> Option<String> {
        self.link_in_view(column, row).map(|link| link.url)
    }

    fn link_in_view(&self, column: usize, row: usize) -> Option<Link> {
        if column >= self.size.columns || row >= self.size.lines {
            return None;
        }
        link::link_at(&self.term, self.point(column, row))
    }

    /// The pointer moved to a cell of the view, or off the cells with
    /// None. The link under it gets a line under it. True when the rows
    /// have to be drawn again, another link is under the pointer now.
    pub fn hover(&mut self, cell: Option<(usize, usize)>) -> bool {
        if cell == self.hover {
            return false;
        }
        let before = self.hovered_link();
        self.hover = cell;
        self.hovered_link() != before
    }

    /// A link is under the pointer.
    pub fn link_hovered(&self) -> bool {
        self.hovered_link().is_some()
    }

    fn hovered_link(&self) -> Option<Link> {
        let (column, row) = self.hover?;
        self.link_in_view(column, row)
    }

    /// Every line printed so far, the scrollback first. The rows the
    /// terminal wrapped a line onto are joined back into it, the blanks
    /// at its end are cut, and the empty rows under the last line are
    /// left out.
    pub fn lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        let last = self.term.last_column();
        let mut lines = Vec::new();
        let mut line = String::new();
        for row in self.term.topmost_line().0..=self.term.bottommost_line().0 {
            let cells = &grid[Line(row)];
            for column in 0..=last.0 {
                let cell = &cells[Column(column)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                line.push(cell.c);
                line.extend(cell.zerowidth().unwrap_or_default());
            }
            if !cells[last].flags.contains(Flags::WRAPLINE) {
                lines.push(take(&mut line).trim_end().to_string());
            }
        }
        if !line.is_empty() {
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// Where the text of line `line` of [`Self::lines`] ends in the view:
    /// the row of the last of its rows, and the column right after its
    /// last char. None while that row is scrolled out of the view.
    pub fn line_end(&self, line: usize) -> Option<(usize, usize)> {
        let grid = self.term.grid();
        let last = self.term.last_column();
        let wraps = |row: i32| grid[Line(row)][last].flags.contains(Flags::WRAPLINE);
        let bottom = self.term.bottommost_line().0;
        let mut row = self.term.topmost_line().0;
        for _ in 0..line {
            while row < bottom && wraps(row) {
                row += 1;
            }
            row += 1;
        }
        while row < bottom && wraps(row) {
            row += 1;
        }
        if row > bottom {
            return None;
        }
        let cells = &grid[Line(row)];
        let end = (0..=last.0)
            .rev()
            .find(|column| cells[Column(*column)].c != ' ')
            .map_or(0, |column| {
                // A wide char takes 2 cells, its spacer counts too.
                column + 1 + usize::from(cells[Column(column)].flags.contains(Flags::WIDE_CHAR))
            });
        let shown = row.checked_add(i32::try_from(grid.display_offset()).ok()?)?;
        let shown = usize::try_from(shown).ok().filter(|shown| *shown < self.size.lines)?;
        Some((shown, end))
    }

    /// Every row of the view, the top one first.
    pub fn rows(&self) -> Vec<Row> {
        let hovered = self.hovered_link();
        let content = self.term.renderable_content();
        let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
        let cursor = (content.cursor.shape != CursorShape::Hidden).then_some(content.cursor.point);
        let mut rows = vec![RowMaker::default(); self.size.lines];

        for cell in content.display_iter {
            let Ok(row) = usize::try_from(cell.point.line.0.saturating_add(offset)) else {
                continue;
            };
            let Some(maker) = rows.get_mut(row) else {
                continue;
            };
            // The second half of a wide char holds nothing of its own.
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let colors = &content.colors;
            let mut text = paint(cell.fg, |index| colors[index]);
            let mut back = paint(cell.bg, |index| colors[index]);
            let dim_named = matches!(cell.fg, Color::Named(name) if is_dim(name));
            if cell.flags.contains(Flags::INVERSE) {
                (text, back) = (back, text);
            }
            let selected = content
                .selection
                .is_some_and(|range| range.contains_cell(&cell, content.cursor.point, content.cursor.shape));
            // The cursor and the selection are the cell with its 2
            // colors changed over.
            if selected || cursor == Some(cell.point) {
                (text, back) = (Paint::Background, Paint::Foreground);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                text = back;
            }
            let look = Look {
                text,
                back,
                bold: cell.flags.contains(Flags::BOLD),
                dim: cell.flags.contains(Flags::DIM) || dim_named,
                lines: Lines {
                    under:   cell.flags.intersects(Flags::ALL_UNDERLINES)
                        || hovered.as_ref().is_some_and(|link| link.contains(cell.point)),
                    through: cell.flags.contains(Flags::STRIKEOUT),
                },
            };
            maker.push(cell.c, cell.zerowidth().unwrap_or_default(), look);
        }

        rows.into_iter().map(RowMaker::done).collect()
    }
}

/// The dim twin of a color a program picked by name.
fn is_dim(name: NamedColor) -> bool {
    (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&(name as usize))
        || name == NamedColor::DimForeground
}

/// The color of a cell. `set` is what a program set a numbered color
/// to, a color it changed counts before the theme.
fn paint(color: Color, set: impl Fn(usize) -> Option<Rgb>) -> Paint {
    let by_number =
        |index: usize, plain: Paint| set(index).map_or(plain, |rgb| Paint::Rgb(rgb.r, rgb.g, rgb.b));
    match color {
        Color::Spec(rgb) => Paint::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => by_number(usize::from(index), Paint::Indexed(index)),
        Color::Named(name) => {
            let index = name as usize;
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                    by_number(NamedColor::Foreground as usize, Paint::Foreground)
                }
                NamedColor::Background => by_number(index, Paint::Background),
                NamedColor::Cursor => by_number(index, Paint::Foreground),
                _ if is_dim(name) => {
                    let plain = index - NamedColor::DimBlack as usize;
                    by_number(plain, Paint::Indexed(u8::try_from(plain).unwrap_or_default()))
                }
                _ => by_number(index, Paint::Indexed(u8::try_from(index).unwrap_or_default())),
            }
        }
    }
}

impl Look {
    /// A cell that shows nothing: no color behind it and no line.
    fn empty(self, char: char) -> bool {
        char == ' ' && self.back == Paint::Background && self.lines == Lines::default()
    }
}

/// Builds a row from its cells, left to right.
#[derive(Clone, Default)]
struct RowMaker {
    text:  String,
    spans: Vec<Span>,
    /// Where the text ends when the empty cells at its end are cut, with
    /// the count of the spans up to there.
    shown: (usize, usize),
}

impl RowMaker {
    /// A wide char is 1 char of the text, the label gives it 2 cells.
    fn push(&mut self, char: char, marks: &[char], look: Look) {
        let start = self.text.len();
        self.text.push(char);
        self.text.extend(marks);
        let end = self.text.len();
        match self.spans.last_mut() {
            Some(span) if span.look == look => span.range.end = end,
            _ => self.spans.push(Span {
                range: start..end,
                look,
            }),
        }
        if !look.empty(char) {
            self.shown = (end, self.spans.len());
        }
    }

    fn done(mut self) -> Row {
        let (end, spans) = self.shown;
        self.text.truncate(end);
        self.spans.truncate(spans);
        if let Some(last) = self.spans.last_mut() {
            last.range.end = end;
        }
        Row {
            text:  self.text,
            spans: self.spans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cells, Paint, Pick, Screen};

    fn screen() -> Screen {
        Screen::new(
            Cells {
                columns: 20,
                lines:   4,
            },
            (8, 16),
        )
    }

    fn no_color(_: usize) -> (u8, u8, u8) {
        (0, 0, 0)
    }

    #[test]
    fn printed_text_is_rows_and_the_cursor_is_a_cell_with_its_colors_changed_over() {
        let mut screen = screen();
        screen.feed(b"ls\r\n\x1b[31mred\x1b[0m ok", no_color);
        let rows = screen.rows();
        assert_eq!(rows[0].text, "ls");
        // The cursor stands after the text, a cell of its own.
        assert_eq!(rows[1].text, "red ok ");
        let looks: Vec<(&str, Paint, Paint)> = rows[1]
            .spans
            .iter()
            .map(|span| (&rows[1].text[span.range.clone()], span.look.text, span.look.back))
            .collect();
        assert_eq!(
            looks,
            [
                ("red", Paint::Indexed(1), Paint::Background),
                (" ok", Paint::Foreground, Paint::Background),
                (" ", Paint::Background, Paint::Foreground),
            ]
        );
        assert_eq!(rows[2].text, "");
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn a_color_behind_empty_cells_is_kept_and_plain_empty_cells_are_cut() {
        let mut screen = screen();
        // 3 cells on green, then the cursor is hidden.
        screen.feed(b"a\x1b[42m   \x1b[0m\x1b[?25l", no_color);
        let row = &screen.rows()[0];
        assert_eq!(row.text, "a   ");
        assert_eq!(row.spans.len(), 2);
        assert_eq!(row.spans[1].look.back, Paint::Indexed(2));
        assert_eq!(row.spans[1].range, 1..4);
    }

    #[test]
    fn a_wide_char_is_1_char_of_the_text() {
        let mut screen = screen();
        screen.feed("a漢b\x1b[?25l".as_bytes(), no_color);
        assert_eq!(screen.rows()[0].text, "a漢b");
    }

    #[test]
    fn a_question_of_a_program_gets_its_answer() {
        let mut screen = screen();
        // Where is the cursor, and what is the color behind the text.
        let asked = screen.feed(b"ab\x1b[6n\x1b]11;?\x07", |_| (0x12, 0x34, 0x56));
        let replies = String::from_utf8(asked.replies).unwrap();
        assert!(replies.starts_with("\x1b[1;3R"), "{replies:?}");
        assert!(replies.contains("rgb:1212/3434/5656"), "{replies:?}");
    }

    #[test]
    fn a_title_and_the_modes_of_a_program_are_read() {
        let mut screen = screen();
        let asked = screen.feed(b"\x1b]0;vim notes\x07\x1b[?1h\x1b[?2004h\x1b[?1049h", no_color);
        assert_eq!(asked.title.as_deref(), Some("vim notes"));
        assert!(screen.application_cursor());
        assert!(screen.bracketed_paste());
        assert!(screen.full_screen());
    }

    #[test]
    fn lines_that_left_the_screen_are_scrolled_back_to() {
        let mut screen = screen();
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[?25l", no_color);
        assert_eq!(screen.rows()[0].text, "two");
        screen.scroll(1);
        assert_eq!(screen.rows()[0].text, "one");
        screen.scroll_to_end();
        assert_eq!(screen.rows()[3].text, "five");
    }

    /// Codex turns on mouse reports and turns off alternate scroll. The
    /// wheel did nothing there before, its log could not be scrolled.
    #[test]
    fn the_wheel_is_a_mouse_report_when_a_program_asks() {
        let mut screen = screen();
        assert_eq!(screen.wheel_report(true, 0, 0), None);
        screen.feed(b"\x1b[?1049h\x1b[?1007l\x1b[?1000h", no_color);
        assert!(!screen.wheel_as_arrows());
        assert_eq!(
            screen.wheel_report(true, 2, 1),
            Some(b"\x1b[M\x60\x23\x22".to_vec())
        );
        screen.feed(b"\x1b[?1006h", no_color);
        assert_eq!(screen.wheel_report(false, 2, 1), Some(b"\x1b[<65;3;2M".to_vec()));
    }

    #[test]
    fn a_drag_selects_cells_and_a_double_click_a_word() {
        let mut screen = screen();
        screen.feed(b"hello wide world\x1b[?25l", no_color);
        screen.select_from(6, 0, Pick::Cells);
        screen.select_to(9, 0);
        assert_eq!(screen.selected_text().as_deref(), Some("wide"));
        let row = &screen.rows()[0];
        let selected = row.spans.iter().find(|span| span.look.back == Paint::Foreground).unwrap();
        assert_eq!(&row.text[selected.range.clone()], "wide");

        screen.select_from(12, 0, Pick::Words);
        assert_eq!(screen.selected_text().as_deref(), Some("world"));
        screen.clear_selection();
        assert_eq!(screen.selected_text(), None);
    }

    // The failure this prevents: an owner that reads the place of an error
    // from the lines gets it cut in 2 where the terminal wrapped it, or
    // misses it once it scrolled away.
    #[test]
    fn the_lines_hold_the_scrollback_with_wrapped_rows_joined() {
        let mut screen = screen();
        screen.feed(
            b"one\r\n\x1b[31merror\x1b[0m: bad\r\n  --> src/a_long_file_name.rs:5:17\r\nfour\r\nfive\r\n",
            no_color,
        );
        assert_eq!(
            screen.lines(),
            [
                "one",
                "error: bad",
                "  --> src/a_long_file_name.rs:5:17",
                "four",
                "five"
            ]
        );
    }

    #[test]
    fn a_line_ends_on_its_last_wrapped_row_and_only_in_view() {
        let mut screen = screen();
        screen.feed(
            b"one\r\nerror: bad\r\n  --> src/a_long_file_name.rs:5:17\r\nfour\r\nfive\r\n",
            no_color,
        );
        // The --> line wrapped onto the top row of the view.
        assert_eq!(screen.line_end(2), Some((0, 14)));
        assert_eq!(screen.line_end(4), Some((2, 4)));
        assert_eq!(screen.line_end(0), None);
        assert_eq!(screen.line_end(9), None);
        screen.scroll(3);
        assert_eq!(screen.line_end(0), Some((0, 3)));
    }
}
