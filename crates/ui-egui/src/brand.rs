//! Brand mark helper for forks. ArtCraft trademarks are removed as required by docs/brand/LICENSE-brand.txt.

/// The wordmark for a dark (`dark = true`) or light UI.
/// For the fork, no ArtCraft wordmark is embedded or returned.
pub fn wordmark(_ctx: &egui::Context, _dark: bool) -> Option<egui::TextureHandle> {
    None
}

/// Draw the wordmark `height` points tall with its left edge at `left_center` (vertically centred).
/// For the fork, no brand mark is drawn.
pub fn paint_wordmark(_ui: &egui::Ui, _left_center: egui::Pos2, _height: f32, _dark: bool) -> Option<egui::Rect> {
    None
}
