//! The screen of a shell on a cell grid. The bytes the shell prints come in
//! with `feed`, the emulator in `screen` turns them into rows of cells, and
//! each row is 1 label on a cell grid with a color behind its cells. The
//! keys go out as bytes through `keys`.

use hilen::{
    Event,
    dispatch::after,
    gm::LossyConvert,
    refs::{Weak, weak_from_ref},
    system::{Clipboard, open_url},
    text::Font,
    ui::{
        Button, Container, CursorIcon, KeyCombo, Keys, Label, NamedKey, Point, RunStyle, Setup, Size, Touch,
        UIColor, UIEvents, UIManager, ViewCallbacks, ViewData, ViewFrame, ViewSubviews, ViewTouch, view,
    },
};
use log::{debug, info, warn};

use crate::{
    keys,
    palette::TerminalPalette,
    screen::{Cells, Lines, Paint, Pick, Row, Screen},
};

/// A screen is never smaller, a program cannot draw on less.
const MIN_CELLS: Cells = Cells {
    columns: 20,
    lines:   4,
};
/// The lines 1 press of Shift with Page Up leaves in view of the page
/// before.
const PAGE_OVERLAP: usize = 2;

/// How a terminal looks. The font has to be a mono font, every cell is as
/// wide as the next.
#[derive(Clone, Copy)]
pub struct TerminalStyle {
    pub font:      Weak<Font>,
    pub bold:      Weak<Font>,
    pub text_size: f32,
    pub palette:   TerminalPalette,
    /// From the edge of the view to the cells.
    pub pad:       f32,
}

/// A button the owner puts right after the text of a printed line, like
/// Apply next to a fix a compiler suggests. It moves with its line when
/// the view scrolls, and hides while the line is out of the view.
#[derive(Clone, Debug, PartialEq)]
pub struct LineButton {
    /// The line in [`TerminalView::lines`].
    pub line:       usize,
    pub title:      String,
    pub color:      UIColor,
    pub text_color: UIColor,
}

/// The size of the screen of a shell, the shell has to hear of every new
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns:     u16,
    pub lines:       u16,
    /// The size of 1 cell in pixels, a program may ask for it.
    pub cell_width:  u16,
    pub cell_height: u16,
}

/// A terminal: the screen of a shell with its colors and its cursor, the
/// keys, a selection with the mouse, copy and paste, the wheel over what
/// scrolled away, and links that open in the browser with a click. It runs no
/// shell, the owner feeds it what a shell prints and sends what it gives out to
/// that shell. `LocalTerminal` does both for a shell on this machine.
#[view]
pub struct TerminalView {
    /// Bytes for the shell: typed keys, a paste, the answers to what the
    /// shell asked and mouse reports.
    pub keys:               Event<Vec<u8>>,
    /// The screen got a size, the first one too.
    pub resized:            Event<TerminalSize>,
    /// A tap on a button of `set_line_buttons`, with its place in that
    /// list.
    pub line_button_tapped: Event<usize>,

    style:        Option<TerminalStyle>,
    /// None until the view has a size.
    screen:       Option<Screen>,
    /// What came in before the screen had a size.
    pending:      Vec<u8>,
    /// What the labels show, a row that did not change is not set again.
    shown:        Vec<Row>,
    labels:       Vec<Weak<Label>>,
    /// The size the cells were fitted into last.
    fitted:       Size,
    /// The screen changed and the labels are not made new yet.
    stale:        bool,
    /// The shell is over, no key goes out.
    ended:        bool,
    /// The cell a press began on, a selection starts when it moves on.
    pressed:      Option<(usize, usize)>,
    /// The link a click with no drag let go on. It opens when no second
    /// click follows, a double click selects a word.
    clicked:      Option<String>,
    /// A link is under the pointer, the board shows the hand.
    on_link:      bool,
    /// What the wheel turned and no line used up yet, in points.
    wheel:        f32,
    line_buttons: Vec<LineButton>,
    /// 1 view for each of `line_buttons`.
    button_views: Vec<Weak<Button>>,

    #[init]
    /// Takes the touches and holds the rows.
    board: Container,
    /// Never shown, the size of 1 cell is asked from it.
    probe: Label,
}

impl Setup for TerminalView {
    fn setup(self: Weak<Self>) {
        self.board.place().back();
        self.board.enable_touch();
        self.board.touch().all.val(move |touch| self.touched(touch));
        self.board.touch().double_tap.val(self, move |touch| self.pick_word(touch));
        self.board.touch().single_tap.val(self, move |_| self.open_clicked());
        self.board.enable_hover();
        self.board.touch().hovered.val(self, move |hovered| {
            if !hovered {
                self.hover(None);
            }
        });
        self.probe.set_hidden(true);

        UIEvents::keyboard_input().val(self, move |char| self.typed(char));
        UIEvents::keyboard_key().val(self, move |key| self.pressed_key(key));
        UIEvents::on_scroll().val(self, move |delta| self.wheeled(delta));
        UIEvents::pointer_moved().val(self, move |point| self.pointer_moved(point));

        let keymap = UIManager::keymap();
        // The engine takes Ctrl for Cmd too, and in a terminal Ctrl with a
        // letter is a key of the shell. So only Cmd pastes and copies.
        keymap.add(self, KeyCombo::cmd('v'), move || {
            if Keys::modifiers().meta() && self.has_keys() {
                self.paste();
            }
        });
        keymap.add(self, KeyCombo::cmd('c'), move || {
            if Keys::modifiers().meta() && self.has_keys() {
                self.copy();
            }
        });
    }
}

impl ViewCallbacks for TerminalView {
    fn update(&mut self) {
        let view = weak_from_ref(self);
        if view.board.size() != view.fitted {
            view.fit();
        }
    }

    fn theme_changed(&mut self) {
        // Every color is the one of the old theme.
        let mut view = weak_from_ref(self);
        view.shown.clear();
        view.draw_soon();
    }
}

impl TerminalView {
    /// The fonts, the size and the colors. Nothing shows before a style is
    /// set.
    pub fn set_style(mut self: Weak<Self>, style: TerminalStyle) {
        self.style = Some(style);
        self.set_color(style.palette.background);
        self.style_label(self.probe);
        for label in self.labels.clone() {
            self.style_label(label);
        }
        self.shown.clear();
        self.fitted = Size::default();
        self.fit();
    }

    /// What the shell printed.
    pub fn feed(mut self: Weak<Self>, bytes: &[u8]) {
        let Some(palette) = self.style.map(|style| style.palette) else {
            self.pending.extend_from_slice(bytes);
            return;
        };
        let Some(screen) = &mut self.screen else {
            self.pending.extend_from_slice(bytes);
            return;
        };
        let asked = screen.feed(bytes, |index| palette.asked(index));
        if !asked.replies.is_empty() {
            self.keys.trigger(asked.replies);
        }
        if let Some(text) = asked.copied
            && let Err(err) = Clipboard::set_text(text)
        {
            warn!("a text the shell copied did not reach the clipboard: {err:#}");
        }
        self.draw_soon();
    }

    /// Buttons right after the text of printed lines, in the place of the
    /// ones before. An empty list takes them all away.
    pub fn set_line_buttons(mut self: Weak<Self>, buttons: Vec<LineButton>) {
        for mut view in std::mem::take(&mut self.button_views) {
            view.remove_from_superview();
        }
        for (index, button) in buttons.iter().enumerate() {
            let view = self.board.add_view::<Button>();
            view.set_text(&button.title);
            view.set_color(button.color);
            view.set_text_color(button.text_color);
            view.on_tap(move || self.line_button_tapped.trigger(index));
            view.set_hidden(true);
            self.button_views.push(view);
        }
        self.line_buttons = buttons;
        self.place_buttons();
    }

    /// An empty screen of the size of now, for a stream of a shell that
    /// starts again from its first byte. The size stays, so `resized` does
    /// not fire.
    pub fn reset(mut self: Weak<Self>) {
        self.set_line_buttons(Vec::new());
        self.pending.clear();
        let cell = self.probe.cell_size();
        if let Some(screen) = &mut self.screen {
            *screen = Screen::new(screen.size(), Self::cell_pixels(cell));
        }
        self.shown.clear();
        self.draw_soon();
    }

    /// Every line printed so far, see [`Screen::lines`]. Empty before the
    /// view has a size.
    pub fn lines(&self) -> Vec<String> {
        self.screen.as_ref().map_or_default(Screen::lines)
    }

    /// The size of the screen, None before the view has one.
    pub fn size(&self) -> Option<TerminalSize> {
        let screen = self.screen.as_ref()?;
        let cell = self.probe.cell_size();
        Some(Self::terminal_size(screen.size(), cell))
    }

    /// The shell is over: no key goes out any more.
    pub fn set_ended(mut self: Weak<Self>, ended: bool) {
        self.ended = ended;
    }

    /// Gives the keys to the terminal, as a click on it does.
    pub fn focus(self: Weak<Self>) {
        self.board.select();
    }

    /// The keys are for the shell: the terminal is the selected view, so
    /// no field and no popup has them, and its shell runs.
    pub fn has_keys(&self) -> bool {
        self.board.is_selected() && !self.ended
    }

    /// A row of the screen: the mono font on a cell grid.
    fn style_label(&self, label: Weak<Label>) {
        let Some(style) = self.style else {
            return;
        };
        label.set_font(style.font).set_text_size(style.text_size).set_cell_grid(true);
    }

    fn pad(&self) -> f32 {
        self.style.map_or(0.0, |style| style.pad)
    }

    /// How many cells fit the view, and the size of 1 in points.
    fn cells(&self) -> (Cells, Size) {
        let cell = self.probe.cell_size();
        let room = self.board.size();
        let pad = self.pad();
        let count = |room: f32, cell: f32| -> usize {
            if cell <= 0.0 {
                return 0;
            }
            ((room - pad * 2.0) / cell).floor().max(0.0).lossy_convert()
        };
        let cells = Cells {
            columns: count(room.width, cell.width).max(MIN_CELLS.columns),
            lines:   count(room.height, cell.height).max(MIN_CELLS.lines),
        };
        (cells, cell)
    }

    fn cell_pixels(cell: Size) -> (u16, u16) {
        let scale = UIManager::scale();
        let pixels = |points: f32| -> u16 {
            let pixels: u32 = (points * scale).round().lossy_convert();
            u16::try_from(pixels).unwrap_or(u16::MAX)
        };
        (pixels(cell.width), pixels(cell.height))
    }

    fn terminal_size(cells: Cells, cell: Size) -> TerminalSize {
        let (cell_width, cell_height) = Self::cell_pixels(cell);
        TerminalSize {
            columns: u16::try_from(cells.columns).unwrap_or(u16::MAX),
            lines: u16::try_from(cells.lines).unwrap_or(u16::MAX),
            cell_width,
            cell_height,
        }
    }

    /// The view got a size or a new one: as many cells as fit, a label per
    /// line, and the owner hears of the new screen for its shell.
    fn fit(mut self: Weak<Self>) {
        self.fitted = self.board.size();
        let (cells, cell) = self.cells();
        if cell.width <= 0.0 || self.style.is_none() || self.board.width() <= 0.0 {
            return;
        }
        self.place_rows(cells, cell);
        match &mut self.screen {
            None => {
                self.screen = Some(Screen::new(cells, Self::cell_pixels(cell)));
                debug!("a terminal of {} by {}", cells.columns, cells.lines);
                self.resized.trigger(Self::terminal_size(cells, cell));
                let pending = std::mem::take(&mut self.pending);
                if !pending.is_empty() {
                    self.feed(&pending);
                }
            }
            Some(screen) if screen.size() == cells => return,
            Some(screen) => {
                screen.resize(cells, Self::cell_pixels(cell));
                debug!("the terminal is {} by {} now", cells.columns, cells.lines);
                self.resized.trigger(Self::terminal_size(cells, cell));
            }
        }
        self.draw_soon();
    }

    /// A label for every line, each 1 cell high, so their colors meet.
    fn place_rows(mut self: Weak<Self>, cells: Cells, cell: Size) {
        while self.labels.len() > cells.lines {
            if let Some(mut label) = self.labels.pop() {
                label.remove_from_superview();
            }
        }
        while self.labels.len() < cells.lines {
            let label = self.board.add_view::<Label>();
            self.style_label(label);
            self.labels.push(label);
        }
        self.shown.truncate(cells.lines);
        // The text of a label starts a little inside it. The label starts
        // that much further left, so the cells start at the pad.
        let inset = self.probe.text_inset();
        let pad = self.pad();
        let columns: f32 = cells.columns.lossy_convert();
        let mut top = pad;
        for label in &self.labels {
            label.set_frame((pad - inset, top, columns * cell.width + inset * 2.0, cell.height));
            top += cell.height;
        }
    }

    /// Draws once after everything of this turn was read, a shell prints
    /// in many small pieces.
    fn draw_soon(mut self: Weak<Self>) {
        if self.stale {
            return;
        }
        self.stale = true;
        after(0.0, move || {
            if self.is_ok() {
                self.stale = false;
                self.draw();
            }
        });
    }

    /// Sets the labels of the rows that changed.
    fn draw(mut self: Weak<Self>) {
        let (Some(screen), Some(style)) = (&self.screen, self.style) else {
            return;
        };
        let rows = screen.rows();
        let on_link = screen.link_hovered();
        for (index, row) in rows.iter().enumerate() {
            let Some(label) = self.labels.get(index).copied() else {
                break;
            };
            if self.shown.get(index) == Some(row) {
                continue;
            }
            Self::draw_row(label, row, &style);
        }
        self.shown = rows;
        self.place_buttons();
        if on_link != self.on_link {
            self.on_link = on_link;
            self.board.set_hover_cursor(if on_link {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
    }

    /// Each button 1 cell after the end of its line, as high as a row.
    fn place_buttons(&self) {
        let (Some(screen), Some(style)) = (&self.screen, self.style) else {
            return;
        };
        let (_, cell) = self.cells();
        let pad = self.pad();
        for (button, view) in self.line_buttons.iter().zip(&self.button_views) {
            let Some((row, column)) = screen.line_end(button.line) else {
                view.set_hidden(true);
                continue;
            };
            let (row, column): (f32, f32) = (row.lossy_convert(), column.lossy_convert());
            view.set_text_size(style.text_size - 2.0);
            view.set_corner_radius(cell.height / 2.0);
            let width = view.content_size().width + cell.width * 2.0;
            view.set_frame((
                pad + (column + 1.0) * cell.width,
                pad + row * cell.height,
                width,
                cell.height,
            ));
            view.set_hidden(false);
        }
    }

    fn draw_row(label: Weak<Label>, row: &Row, style: &TerminalStyle) {
        let palette = style.palette;
        label.set_text(&row.text);
        label.set_color_runs(row.spans.iter().map(|span| {
            (
                span.range.clone(),
                UIColor::from(palette.color(span.look.text, span.look.dim)),
            )
        }));
        label.set_background_runs(
            row.spans.iter().filter(|span| span.look.back != Paint::Background).map(|span| {
                (
                    span.range.clone(),
                    UIColor::from(palette.color(span.look.back, false)),
                )
            }),
        );
        label.set_font_runs(
            row.spans
                .iter()
                .filter(|span| span.look.bold || span.look.lines != Lines::default())
                .map(|span| {
                    (
                        span.range.clone(),
                        RunStyle {
                            font: span.look.bold.then_some(style.bold),
                            underline: span.look.lines.under,
                            strikethrough: span.look.lines.through,
                            ..RunStyle::default()
                        },
                    )
                }),
        );
    }

    fn send(self: Weak<Self>, keys: Vec<u8>) {
        self.keys.trigger(keys);
    }

    /// A char was typed anywhere in the app.
    fn typed(self: Weak<Self>, char: char) {
        if !self.has_keys() {
            return;
        }
        let Some(keys) = keys::typed(char, Keys::modifiers()) else {
            return;
        };
        self.follow_the_shell();
        self.send(keys);
    }

    /// A key with a name was pressed anywhere in the app.
    fn pressed_key(mut self: Weak<Self>, key: NamedKey) {
        if !self.has_keys() {
            return;
        }
        let modifiers = Keys::modifiers();
        let Some(screen) = &mut self.screen else {
            return;
        };
        // Shift with Page Up and Page Down turns the pages of what scrolled
        // away, like in every terminal. A program that draws the whole
        // screen has no such pages and gets the key.
        let page = match key {
            NamedKey::PageUp => 1,
            NamedKey::PageDown => -1,
            _ => 0,
        };
        if page != 0 && modifiers.shift() && !screen.full_screen() {
            let lines = screen.size().lines.saturating_sub(PAGE_OVERLAP).max(1);
            screen.scroll(page * i32::try_from(lines).unwrap_or(1));
            self.draw_soon();
            return;
        }
        let Some(keys) = keys::named(key, modifiers, screen.application_cursor()) else {
            return;
        };
        self.follow_the_shell();
        self.send(keys);
    }

    /// A key goes to the shell: the view is back at the newest lines and
    /// nothing is selected, as in every terminal.
    fn follow_the_shell(mut self: Weak<Self>) {
        if let Some(screen) = &mut self.screen {
            screen.scroll_to_end();
            screen.clear_selection();
            self.draw_soon();
        }
    }

    fn paste(self: Weak<Self>) {
        let text = match Clipboard::get_text() {
            Ok(text) => text,
            Err(err) => {
                warn!("the clipboard gave no text to paste: {err:#}");
                return;
            }
        };
        let Some(screen) = &self.screen else {
            return;
        };
        debug!("pasting {} bytes into the terminal", text.len());
        let keys = keys::pasted(&text, screen.bracketed_paste());
        self.follow_the_shell();
        self.send(keys);
    }

    fn copy(self: Weak<Self>) {
        let Some(text) = self.screen.as_ref().and_then(Screen::selected_text) else {
            return;
        };
        debug!("copying {} bytes of the terminal", text.len());
        if let Err(err) = Clipboard::set_text(text) {
            warn!("the selected text did not reach the clipboard: {err:#}");
        }
    }

    /// The cell under a point of the board.
    fn cell_at(&self, point: Point) -> (usize, usize) {
        let cell = self.probe.cell_size();
        let pad = self.pad();
        let index = |at: f32, cell: f32| -> usize {
            if cell <= 0.0 {
                return 0;
            }
            ((at - pad) / cell).floor().max(0.0).lossy_convert()
        };
        (index(point.x, cell.width), index(point.y, cell.height))
    }

    /// A press and a drag select text, a plain click takes a selection
    /// away and keeps the link under it for `open_clicked`.
    fn touched(mut self: Weak<Self>, touch: Touch) {
        let at = self.cell_at(touch.position);
        let mut pressed = self.pressed;
        let mut clicked = None;
        let Some(screen) = &mut self.screen else {
            return;
        };
        if touch.is_began() {
            screen.clear_selection();
            pressed = Some(at);
        } else if touch.is_moved() {
            if let Some(from) = pressed.take_if(|from| *from != at) {
                screen.select_from(from.0, from.1, Pick::Cells);
            }
            if pressed.is_none() {
                screen.select_to(at.0, at.1);
            }
        } else {
            clicked = pressed.take().and_then(|(column, row)| screen.link_at(column, row));
        }
        self.pressed = pressed;
        self.clicked = clicked;
        self.draw_soon();
    }

    /// A click with no drag and no second click after it: its link opens
    /// in the browser.
    fn open_clicked(mut self: Weak<Self>) {
        let Some(url) = self.clicked.take() else {
            return;
        };
        // The query of an address can hold a token, it stays out of the log.
        let address = url.split(['?', '#']).next().unwrap_or_default();
        info!("opening the link {address} of the terminal");
        if let Err(err) = open_url(&url) {
            warn!("the link {address} of the terminal did not open: {err:#}");
        }
    }

    /// The pointer moved over the window. Over the cells the link under it
    /// gets a line under it.
    fn pointer_moved(self: Weak<Self>, point: Point) {
        let frame = self.absolute_frame();
        let cell =
            (!self.is_hidden_in_tree() && frame.contains(point)).then(|| self.cell_at(point - frame.origin));
        self.hover(cell);
    }

    fn hover(mut self: Weak<Self>, cell: Option<(usize, usize)>) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        if screen.hover(cell) {
            self.draw_soon();
        }
    }

    /// A double click selects the word under it.
    fn pick_word(mut self: Weak<Self>, touch: Touch) {
        let (column, row) = self.cell_at(touch.position);
        if let Some(screen) = &mut self.screen {
            screen.select_from(column, row, Pick::Words);
            self.draw_soon();
        }
    }

    /// The wheel turned somewhere in the app. Over the terminal it moves
    /// the view over what scrolled away, and inside a program that draws
    /// the whole screen it is the arrow keys of that program.
    fn wheeled(mut self: Weak<Self>, delta: Point) {
        if self.is_hidden_in_tree() || !self.absolute_frame().contains(UIManager::cursor_position()) {
            return;
        }
        let cell = self.probe.cell_size().height;
        if cell <= 0.0 {
            return;
        }
        self.wheel += delta.y;
        let lines = (self.wheel / cell).trunc();
        if lines == 0.0 {
            return;
        }
        self.wheel -= lines * cell;
        let lines: i32 = lines.lossy_convert();
        let cell_at = self.cell_at(UIManager::cursor_position() - self.absolute_frame().origin);
        let Some(screen) = &mut self.screen else {
            return;
        };
        let (column, row) = cell_at;
        let presses = usize::try_from(lines.unsigned_abs()).unwrap_or(1);
        if let Some(report) = screen.wheel_report(lines > 0, column, row) {
            self.send(report.repeat(presses));
        } else if screen.wheel_as_arrows() {
            let key = if lines > 0 {
                NamedKey::ArrowUp
            } else {
                NamedKey::ArrowDown
            };
            let Some(code) = keys::named(key, Keys::modifiers(), screen.application_cursor()) else {
                return;
            };
            self.send(code.repeat(presses));
        } else if !screen.full_screen() {
            screen.scroll(lines);
            self.draw_soon();
        }
    }
}
