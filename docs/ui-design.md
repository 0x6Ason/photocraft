# UI design system

## Themes

| Theme | Intent |
|---|---|
| **Pro** (default) | Photoshop-style Spectrum dark: flat charcoal panels (#323232), dark tab strips, Spectrum blue accent (#378ef0), pill buttons, checkboxes, compact 12 px type |
| Studio | Photon-style: near-black, rounded cards, pill tabs, violet accent, toggles |
| Studio Light | Studio on light surfaces |
| Classic | Windows-2000 bevels, square corners, navy selection |

Switch themes with the sun icon, Window → Theme, or `ui.set {"theme":"classic"}` over the control channel.

## Rules

- **Colours and radii come from tokens.** Read them with `Tokens::get(ctx)`; never hard-code a colour in a widget.
- **Shared widgets live in `widgets.rs`:** `card` (panel group; Pro renders a Photoshop tab strip), `value_field`, `slider`/`slider_row`, `toggle`/`checkbox`, `primary_button`/`secondary_button`, `dropdown` (with a chevron icon), `hairline`/`vline`.
- **Icons:** Lucide SVGs in `assets/icons` (ISC licence), embedded via `icon_data.rs`. Regenerate that file when you add icons.
- **Fonts:** Inter (UI) and JetBrains Mono (numbers), both OFL. Named families `medium` and `semibold` are available via `theme::medium()` and `theme::semibold()`.
- **Photoshop layout grammar (Pro):**
  - Essentials dock order: Color | Swatches, then Properties | Adjustments, then Layers | Channels | Paths (Layers fills the remaining height).
  - Options-bar labels end with a colon ("Size:").
  - Document tabs read "name @ 12.5% (RGB/8)".
  - Toolbar tool groups carry a corner triangle.
- **Verify every visual change** with `ui.screenshot`, at several window sizes and in every theme.
