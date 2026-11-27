//! A terminal with a shell or another program of this machine.

use std::path::{Path, PathBuf};

use hilen::{
    Event,
    dispatch::on_main,
    refs::Weak,
    ui::{Setup, ViewData, view},
};
use log::warn;

use crate::{LineButton, LocalShell, TerminalSize, TerminalStyle, TerminalView, shell::CTRL_C};

/// What starts once the view has a size.
struct Start {
    folder:  PathBuf,
    /// A program and its arguments, None is the login shell.
    command: Option<Vec<String>>,
}

/// A `TerminalView` with the login shell of the user or another program in
/// it. It starts in its folder once the view has a size, and ends with the
/// view.
#[view]
pub struct LocalTerminal {
    /// The shell or the program ended, by `exit` or by itself, with its
    /// exit code. One that a signal ended has none.
    pub ended:              Event<Option<u32>>,
    /// A tap on a button of `set_line_buttons`, with its place in that
    /// list.
    pub line_button_tapped: Event<usize>,
    /// A click on a place in a file, see [`TerminalView::place_tapped`].
    pub place_tapped:       Event<String>,

    waiting: Option<Start>,
    shell:   Option<LocalShell>,

    #[init]
    view: TerminalView,
}

impl Setup for LocalTerminal {
    fn setup(self: Weak<Self>) {
        self.view.place().back();
        self.view.keys.val(move |keys| self.write(&keys));
        self.view.resized.val(move |size| self.resized(size));
        self.view
            .line_button_tapped
            .val(move |index| self.line_button_tapped.trigger(index));
        self.view.place_tapped.val(move |place| self.place_tapped.trigger(place));
    }
}

impl LocalTerminal {
    pub fn set_style(self: Weak<Self>, style: TerminalStyle) {
        self.view.set_style(style);
    }

    /// Starts the shell in `folder`, now or when the view gets its size.
    pub fn start(self: Weak<Self>, folder: &Path) {
        self.start_when_sized(Start {
            folder:  folder.to_path_buf(),
            command: None,
        });
    }

    /// Runs `command`, a program and its arguments, in `folder` in place
    /// of the shell, now or when the view gets its size. The keys go to the
    /// program. The terminal is over when the program ends.
    pub fn run(self: Weak<Self>, folder: &Path, command: &[String]) {
        self.start_when_sized(Start {
            folder:  folder.to_path_buf(),
            command: Some(command.to_vec()),
        });
    }

    /// The shell or the program runs and has not ended.
    pub fn is_running(&self) -> bool {
        self.shell.is_some()
    }

    /// Ctrl C for what runs in the terminal, as the key sends it.
    pub fn interrupt(self: Weak<Self>) {
        self.write(&[CTRL_C]);
    }

    /// A text of the owner on the screen, like a note on how a program
    /// ended. It takes the escape codes a program prints, and a new line is
    /// `\r\n`.
    pub fn print(self: Weak<Self>, text: &str) {
        self.view.feed(text.as_bytes());
    }

    /// Every line the shell or the program printed so far, the scrollback
    /// first, with the rows the terminal wrapped joined. All of it is there
    /// when `ended` fires.
    pub fn lines(&self) -> Vec<String> {
        self.view.lines()
    }

    /// Buttons right after the text of printed lines, see
    /// [`TerminalView::set_line_buttons`].
    pub fn set_line_buttons(self: Weak<Self>, buttons: Vec<LineButton>) {
        self.view.set_line_buttons(buttons);
    }

    /// Gives the keys to the terminal, as a click on it does.
    pub fn focus(self: Weak<Self>) {
        self.view.focus();
    }

    pub fn has_keys(&self) -> bool {
        self.view.has_keys()
    }

    fn resized(self: Weak<Self>, size: TerminalSize) {
        match &self.shell {
            Some(shell) => {
                if let Err(err) = shell.resize(size) {
                    warn!("{err:#}");
                }
            }
            None => self.start_shell(size),
        }
    }

    fn start_when_sized(mut self: Weak<Self>, start: Start) {
        self.waiting = Some(start);
        if let Some(size) = self.view.size() {
            self.start_shell(size);
        }
    }

    fn start_shell(mut self: Weak<Self>, size: TerminalSize) {
        let Some(start) = self.waiting.take() else {
            return;
        };
        let printed = move |bytes: Vec<u8>| {
            on_main(move || {
                if self.is_ok() {
                    self.view.feed(&bytes);
                }
            });
        };
        let ended = move |code: Option<u32>| {
            on_main(move || {
                if self.is_ok() {
                    let mut this = self;
                    this.shell = None;
                    this.view.set_ended(true);
                    this.ended.trigger(code);
                }
            });
        };
        let started = match &start.command {
            Some(command) => LocalShell::run(command, &start.folder, size, printed, ended),
            None => LocalShell::start(&start.folder, size, printed, ended),
        };
        match started {
            Ok(shell) => {
                self.view.set_ended(false);
                self.shell = Some(shell);
            }
            Err(err) => {
                warn!("{err:#}");
                // The reason can have several lines, and a terminal goes back
                // to the start of a line only on `\r`.
                let reason = format!("{err:#}").replace('\n', "\r\n");
                self.view.feed(format!("{reason}\r\n").as_bytes());
                self.view.set_ended(true);
            }
        }
    }

    fn write(mut self: Weak<Self>, keys: &[u8]) {
        if let Some(shell) = &mut self.shell
            && let Err(err) = shell.write(keys)
        {
            warn!("{err:#}");
        }
    }
}
