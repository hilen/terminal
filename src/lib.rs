//! A terminal for hilen apps: the screen of a shell with its colors, its
//! keys and a selection, and on a desktop a shell on this machine to show
//! in it.

#![allow(incomplete_features)]
#![feature(specialization)]
#![feature(arbitrary_self_types)]

mod keys;
mod link;
mod palette;
mod screen;
mod view;

/// A shell on this machine needs a pty, a phone and a browser have none.
#[cfg(not(any(target_os = "ios", target_os = "android", target_arch = "wasm32")))]
mod local;
#[cfg(not(any(target_os = "ios", target_os = "android", target_arch = "wasm32")))]
mod shell;

#[cfg(not(any(target_os = "ios", target_os = "android", target_arch = "wasm32")))]
pub use local::LocalTerminal;
pub use palette::TerminalPalette;
#[cfg(not(any(target_os = "ios", target_os = "android", target_arch = "wasm32")))]
pub use shell::LocalShell;
pub use view::{TerminalSize, TerminalStyle, TerminalView};
