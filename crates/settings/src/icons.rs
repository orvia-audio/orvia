//! Embedded Tabler outline SVGs used by the settings window.
use gpui::{AssetSource, Result, SharedString, Styled, Svg, px, rgb, svg};
use rust_embed::RustEmbed;
use std::borrow::Cow;

/// Serves icon paths from the executable, including when launched from Start.
#[derive(RustEmbed)]
#[folder = "../../assets/tabler/outline/"]
pub(super) struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|file| file.data))
    }

    fn list(&self, prefix: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|path| path.starts_with(prefix))
            .map(|path| SharedString::from(path.into_owned()))
            .collect())
    }
}

/// Draw a decorative glyph beside its localized label without adding a tab stop.
pub(super) fn glyph(path: &'static str, size: f32, color: u32) -> Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
}
