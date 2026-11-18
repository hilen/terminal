//! A terminal with a shell on this machine.

use std::path::{Path, PathBuf};

use hilen::{
    Event,
    dispatch::on_main,
    refs::Weak,
    ui::{Setup, ViewData, view},
};
use log::warn;

use crate::{LocalShell, TerminalSize, TerminalStyle, TerminalView};

/// A `TerminalView` with the login shell of the user in it. The shell
/// starts in its folder once the view has a size, and ends with the view.
#[view]
pub struct LocalTerminal {
    /// The shell ended, by `exit` or by itself.
    pub ended: Event,

    folder: Option<PathBuf>,
    shell:  Option<LocalShell>,

    #[init]
    view: TerminalView,
}

impl Setup for LocalTerminal {
    fn setup(self: Weak<Self>) {
        self.view.place().back();
        self.view.keys.val(move |keys| self.write(&keys));
        self.view.resized.val(move |size| self.resized(size));
    }
}

impl LocalTerminal {
    pub fn set_style(self: Weak<Self>, style: TerminalStyle) {
        self.view.set_style(style);
    }

    /// Starts the shell in `folder`, now or when the view gets its size.
    pub fn start(mut self: Weak<Self>, folder: &Path) {
        self.folder = Some(folder.to_path_buf());
        if let Some(size) = self.view.size() {
            self.start_shell(size);
        }
    }

    /// The shell runs and has not ended.
    pub fn is_running(&self) -> bool {
        self.shell.is_some()
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

    fn start_shell(mut self: Weak<Self>, size: TerminalSize) {
        let Some(folder) = self.folder.take() else {
            return;
        };
        let printed = move |bytes: Vec<u8>| {
            on_main(move || {
                if self.is_ok() {
                    self.view.feed(&bytes);
                }
            });
        };
        let ended = move || {
            on_main(move || {
                if self.is_ok() {
                    let mut this = self;
                    this.shell = None;
                    this.view.set_ended(true);
                    this.ended.trigger(());
                }
            });
        };
        match LocalShell::start(&folder, size, printed, ended) {
            Ok(shell) => {
                self.view.set_ended(false);
                self.shell = Some(shell);
            }
            Err(err) => {
                warn!("{err:#}");
                self.view.feed(format!("{err:#}\r\n").as_bytes());
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
