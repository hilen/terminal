//! The colors of a terminal: the 16 a program picks by name, in a light and
//! a dark form, and the 240 by number, which are the same in both.

use hilen::{
    gm::LossyConvert,
    ui::{Color, DynamicColor},
};

use crate::screen::Paint;

const fn pair(light: &str, dark: &str) -> DynamicColor {
    DynamicColor::new(Color::hex(light), Color::hex(dark))
}

/// The 6 steps of each part of a color of the 216 color cube.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// How much of its color a dim text keeps.
const DIM: f32 = 0.6;

/// The colors of a terminal. The default is Atelier Dune Light in the light
/// theme and Bamboo in the dark one, the colors of banda.
#[derive(Clone, Copy, Debug)]
pub struct TerminalPalette {
    pub text:       DynamicColor,
    pub background: DynamicColor,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the
    /// bright form of each.
    pub named:      [DynamicColor; 16],
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self {
            text:       pair("#6e6b5e", "#f1e9d2"),
            background: pair("#fefbec", "#252623"),
            named:      [
                pair("#3b3a32", "#1c1e1b"),
                pair("#d73737", "#e75a7c"),
                pair("#60ac39", "#8fb573"),
                pair("#ae9513", "#dbb651"),
                pair("#6684e1", "#57a5e5"),
                pair("#b854d4", "#aaaaff"),
                pair("#1fad83", "#70c2be"),
                pair("#a8a597", "#cfc8b4"),
                pair("#8a8777", "#6f6d62"),
                pair("#e04f4f", "#f27a96"),
                pair("#74c048", "#a6cc88"),
                pair("#c9ad1c", "#ecc96a"),
                pair("#7f9af0", "#78b9f0"),
                pair("#cb6ee4", "#c2c2ff"),
                pair("#2ec79a", "#8ad6d2"),
                pair("#fefbec", "#f1e9d2"),
            ],
        }
    }
}

fn parts(red: u8, green: u8, blue: u8) -> Color {
    Color::rgba(
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        1.0,
    )
}

impl TerminalPalette {
    /// A color by its number, 0 to 255, in the theme of now.
    fn numbered(&self, index: u8) -> Color {
        match index {
            0..=15 => self.named[usize::from(index)].resolve(),
            16..=231 => {
                let cube = usize::from(index - 16);
                parts(CUBE[cube / 36], CUBE[cube / 6 % 6], CUBE[cube % 6])
            }
            232..=255 => {
                let gray = 8 + 10 * (index - 232);
                parts(gray, gray, gray)
            }
        }
    }

    /// The color of a cell in the theme of now.
    pub(crate) fn color(&self, paint: Paint, dim: bool) -> Color {
        let color = match paint {
            Paint::Foreground => self.text.resolve(),
            Paint::Background => self.background.resolve(),
            Paint::Indexed(index) => self.numbered(index),
            Paint::Rgb(red, green, blue) => parts(red, green, blue),
        };
        if dim {
            Color::rgba(color.r, color.g, color.b, color.a * DIM)
        } else {
            color
        }
    }

    /// The parts of a color by the number a program asks with: 0 to 255,
    /// then the text color and the color behind it. For a program that asks
    /// the terminal which colors it has, to pick a light or a dark look.
    pub(crate) fn asked(&self, index: usize) -> (u8, u8, u8) {
        let color = match u8::try_from(index) {
            Ok(index) => self.numbered(index),
            Err(_) if index == 257 => self.background.resolve(),
            Err(_) => self.text.resolve(),
        };
        let part = |value: f32| -> u8 { (value.clamp(0.0, 1.0) * 255.0).round().lossy_convert() };
        (part(color.r), part(color.g), part(color.b))
    }
}
