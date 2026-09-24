//! Deterministic environment setup for Hydrolysis-backed tests.

use waterui::Environment;

/// Installs the framework default colour and font tokens required by
/// Hydrolysis rendering.
///
/// This is the same token set the runtimes install through
/// [`crate::theme::install_default_tokens`]. It is deliberately *not* a
/// [`crate::Style`]: it carries no widget theme, so tests that mount widgets
/// on the rendered runtime must still supply a real style.
pub fn install_theme(env: &mut Environment) {
    crate::theme::install_default_tokens(env);
}
