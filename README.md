# terminal

A terminal for apps on [hilen](https://github.com/hilen/hilen): the screen of a shell
with its colors and its cursor, the keys, a selection with the mouse, copy and paste,
the wheel over what scrolled away, and links that open in the browser. On a desktop it also starts the login shell of
the user in a pty, or runs another program there. `vim`, `htop` and `ssh` work in it.

The emulator is the [alacritty_terminal](https://github.com/alacritty/alacritty/tree/master/alacritty_terminal)
crate. Each row of the screen is 1 hilen label on a cell grid, with a color behind its
cells.

## Use

```toml
terminal = { git = "https://github.com/hilen/terminal", rev = "<full sha>" }
```

- `TerminalView` shows a shell that runs anywhere. The app feeds it what the shell
  prints with `feed`, and sends what its `keys` event gives out to that shell. Its
  `resized` event fires when the screen gets a size, the first one too, and the shell
  has to hear of it. `reset` gives an empty screen of the same size, for a stream that
  starts again from its first byte. banda shows its shells on other machines this way.
- `LocalTerminal` is a `TerminalView` with the login shell of this machine in it, made
  by `LocalShell`. `start(folder)` starts the shell in that folder once the view has a
  size. Dropping it closes the pty, which hangs the shell up, like a closed terminal
  window. codeloom shows its terminal panel this way. A phone and a browser build have
  no `LocalTerminal` and no `LocalShell`.
- `LocalTerminal` also runs a program in place of the shell: `run(folder, command)`
  takes the program and its arguments and finds it on the `PATH` of the app. The pty is
  its terminal, so it prints its colors and reads the keys. `ended` fires with the exit
  code of the shell or the program, with none when a signal ended it. `interrupt()`
  sends Ctrl C to what runs, the way the key does. `print(text)` puts a text of the app
  on the screen, like a note on how the program ended. It takes the escape codes a
  program prints, and a new line is `\r\n`. codeloom runs the program of the open file
  this way.
- `lines()` of `LocalTerminal` and `TerminalView` gives every line printed so far, the
  scrollback first, as plain text with no escape codes. A line the terminal wrapped onto
  more rows comes back whole. Everything is there when `ended` fires, so the app can
  read what the program said at its end. codeloom finds the first error of a build this
  way.
- `TerminalStyle` holds a mono font, the bold font, the text size, the gap to the edge
  and a `TerminalPalette`. The default palette is Atelier Dune Light in the light theme
  and Bamboo in the dark one.
- A click on a link opens it in the browser, with `open_url` of hilen, which works on
  every platform. A link is an `http` or `https` address a program printed, found by
  [linkify](https://github.com/robinst/linkify) in the whole line, so an address the
  terminal wrapped onto more rows opens whole. A link a program marked with OSC 8, the
  escape code of a link in a terminal, opens its own address. Any other kind, like
  `javascript:` or `file:`, does not open. The link opens only when no second click
  follows, about 250 ms later, so a double click still selects a word, and a drag still
  selects text. The link under the pointer gets a line under it and a hand cursor.
- Copy is Cmd C and paste Cmd V. The engine also takes Ctrl for Cmd, and a shell needs
  Ctrl with a letter, so an app that binds Cmd keys checks `Keys::modifiers().meta()`
  while the terminal has the keys, `has_keys`.

## The hilen pin

The views of this crate are hilen views, so an app that uses it pins the same hilen
commit as this crate. Two commits give Cargo 2 copies of hilen, and the views of this
crate then fit in none of the windows of the app. A hilen update moves this crate
first, then the apps.
