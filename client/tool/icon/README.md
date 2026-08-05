# Launcher icon art

The app icon is the same mark as the in-app `BrandMark` widget
(`lib/widgets/brand_mark.dart`): three rank chevrons in the brand gradient on
the deck's slate tile. Colours come from `BrandAssets` (`lib/theme/brand_assets.dart`) — `tileTop` /
`tileBottom` for the tile, `chevronGradient` for the chevrons.

| File | Role |
|------|------|
| `icon_full.svg` | Android legacy (pre-adaptive) launcher icon — tile *and* chevrons |
| `icon_background.svg` | Android adaptive background layer: the tile gradient, full-bleed |
| `icon_foreground.svg` | Android adaptive foreground layer: the chevrons, transparent |
| `icon_ios.svg` | iOS app icon — full-bleed, opaque |
| `icon_macos.svg` | macOS app icon — inset on Apple's 824/1024 grid |

The SVGs are the masters; the PNGs next to them are renders, and are what
`flutter_launcher_icons` (configured in `pubspec.yaml`) actually consumes.

### Why the Apple platforms get their own masters

They cannot share `icon_full.svg`, because the three platforms disagree about
who owns the rounded corner:

- **iOS** masks the icon to its own superellipse (~22.4% radius) and forbids an
  alpha channel. Our 26% radius sits *outside* that mask, so reusing
  `icon_full` would show the flattened background as four corner wedges.
  `icon_ios.svg` is therefore square, full-bleed, and drops the white hairline
  (which traced an edge iOS now clips).
- **macOS** masks nothing and insets nothing, so the file must do both. Since
  Big Sur the rounded square occupies 824×824 of a 1024×1024 canvas with a
  185.4px radius and a transparent margin — that margin is what makes the icon
  match its neighbours in the Dock. `icon_macos.svg` scales the shared
  1024-space geometry by 824/1024 rather than pre-multiplying coordinates, so
  the chevrons stay one source of truth with the other masters.

`test/theme/launcher_icon_test.dart` pins both rules, so collapsing these files
back into `icon_full.svg` fails the suite rather than shipping a wrong icon.

## Regenerating

After editing an SVG, re-render its PNG and rebuild the platform resources:

```sh
cd client/tool/icon
for f in icon_full icon_background icon_foreground icon_ios icon_macos; do
  rsvg-convert -w 1024 -h 1024 $f.svg -o $f.png
done
cd ../.. && dart run flutter_launcher_icons
```

`rsvg-convert` is not in the dev shell; get it ad hoc with
`nix shell --inputs-from . nixpkgs#librsvg` (from the repo root). `--inputs-from`
borrows the flake's pinned nixpkgs instead of the `nixpkgs-unstable` channel,
which matters on Intel macs — unstable has dropped `x86_64-darwin`, so the
channel form fails to evaluate there.

`test/theme/launcher_icon_test.dart` pins the SVG colours to `BrandAssets` so a
palette retune can't silently leave the launcher icon on the old scheme.

## Sizing

`mipmap-anydpi-v26/ic_launcher.xml` insets the foreground layer by 16%, and the
launcher mask keeps the central 72/108 of the canvas. `icon_foreground.svg`
therefore scales the chevrons by 0.98 so they land at the same size, relative to
the visible tile, as in `icon_full.svg`.
