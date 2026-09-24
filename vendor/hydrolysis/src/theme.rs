//! Framework default theme tokens.
//!
//! [`install_default_tokens`] installs the colour and font tokens every
//! Hydrolysis runtime needs before a [`Style`](crate::Style) applies its own
//! tokens on top. It is installed by both runtimes — the rendered runtime
//! before `style.install_tokens(&mut env)`, and the semantic runtime as its
//! only token source — so widgets resolve the same framework defaults whether
//! or not a style package is present.
//!
//! These tokens are *not* a theme: they carry no widget theme and no
//! presentation decisions beyond the framework's baseline palette and type
//! scale. A `Style` may overwrite any of them.

use waterui::{
    Environment, Plugin,
    color::{ResolvedColor, Srgb},
    theme::{ColorScheme, ColorSettings, FontSettings, Theme},
};

fn color(rgb: u32) -> ResolvedColor {
    ResolvedColor::from_srgb(Srgb::from_u32(rgb))
}

/// Installs the framework default colour and font tokens into `env`.
///
/// Call order matters: install these before `Style::install_tokens` so a
/// style's tokens win over the framework defaults.
pub fn install_default_tokens(env: &mut Environment) {
    Theme::new()
        .color_scheme(ColorScheme::Light)
        .colors(
            ColorSettings::new()
                .background(color(0xFF_FF_FF))
                .surface(color(0xFF_FF_FF))
                .surface_variant(color(0xF3_F4_F6))
                .border(color(0xD1_D5_DB))
                .foreground(color(0x11_18_27))
                .muted_foreground(color(0x4B_55_63))
                .accent(color(0x25_63_EB))
                .accent_container(color(0xDB_EA_FE))
                .accent_foreground(color(0xFF_FF_FF))
                .tertiary(color(0x7C_3A_ED))
                .tertiary_container(color(0xED_E9_FE))
                .selection_container(color(0x25_63_EB))
                .selection_foreground(color(0xFF_FF_FF))
                .error(color(0xDC_26_26))
                .error_foreground(color(0xFF_FF_FF)),
        )
        .fonts(FontSettings::default_scale())
        .install(env);
}
