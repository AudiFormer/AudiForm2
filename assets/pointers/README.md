# Theme pointers

Built into the app by `app/src/pointers.rs`, which draws them as a software
pointer over the window (View > Theme Pointers turns them off).

| Files | Themes | Source | Licence |
|---|---|---|---|
| `glass-*.png` | Aqua Aero, Meadow Aero, Moonlight Aero, Prism | The glass pointer SVGs (`cursor`, `cursor-text`, ...) in Audiform's theme files (`src/themes/aqua.audtheme` and the other Aero themes), made in the user's own commits | AudiForm 2 License |
| `phosphor-*.png` | Phosphor | The pixel pointer SVGs (`phosphor-arrow`, ...) and Phosphor's own trim pointers in Audiform's `src/themes/phosphor.audtheme`, made in the user's own commits | AudiForm 2 License |
| `mall-*.png` | Mall Pastel '95, Mall Pastel '95 Night | The `mallpastel-*` pointer SVGs and the theme's trim pointers in Audiform's `src/themes/mallpastel.audtheme` (the Night theme uses the same files), made in the user's own commits | AudiForm 2 License |

Each SVG was rasterised with headless Edge at 2x (`@2x.png`) and 3x
(`@3x.png`) its CSS size, on a transparent background. The hotspots, in CSS
pixels of the 1x picture, come from the themes' `cursor: url(...) X Y` rules
(arrows) and their `pointers` objects (every other kind); they are in
`pointers.rs`.

No GeekCast or AAE material is used.
