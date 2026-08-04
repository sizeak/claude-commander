# Launcher icon art

The app icon is the same mark as the in-app `BrandMark` widget
(`lib/widgets/brand_mark.dart`): three rank chevrons in the brand gradient on
the deck's slate tile. Colours come from `BrandAssets` (`lib/theme/brand_assets.dart`) — `tileTop` /
`tileBottom` for the tile, `chevronGradient` for the chevrons.

| File | Role |
|------|------|
| `icon_full.svg` | Legacy (pre-adaptive) launcher icon — tile *and* chevrons |
| `icon_background.svg` | Adaptive background layer: the tile gradient, full-bleed |
| `icon_foreground.svg` | Adaptive foreground layer: the chevrons, transparent |

The SVGs are the masters; the PNGs next to them are renders, and are what
`flutter_launcher_icons` (configured in `pubspec.yaml`) actually consumes.

## Regenerating

After editing an SVG, re-render its PNG and rebuild the Android resources:

```sh
cd client/tool/icon
for f in icon_full icon_background icon_foreground; do
  rsvg-convert -w 1024 -h 1024 $f.svg -o $f.png
done
cd ../.. && flutter pub run flutter_launcher_icons
```

`test/theme/launcher_icon_test.dart` pins the SVG colours to `BrandAssets` so a
palette retune can't silently leave the launcher icon on the old scheme.

## Sizing

`mipmap-anydpi-v26/ic_launcher.xml` insets the foreground layer by 16%, and the
launcher mask keeps the central 72/108 of the canvas. `icon_foreground.svg`
therefore scales the chevrons by 0.98 so they land at the same size, relative to
the visible tile, as in `icon_full.svg`.
