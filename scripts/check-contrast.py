#!/usr/bin/env python3
"""WCAG contrast ratios for the Cleat palette, read from the stylesheet itself.

The palette comment in `tauri-rs/src/styles.css` claims every text token clears
AA on the surfaces it is drawn on. This is what checks that claim, so it cannot
quietly stop being true: it parses the OKLCH tokens, converts them to linear
sRGB through Oklab, and prints the ratio of every foreground against every
surface, per theme.

    python3 scripts/check-contrast.py            # report both themes
    python3 scripts/check-contrast.py --strict   # exit 1 if a used pair fails

`--strict` judges only the pairs the UI actually renders (see USED_ON below);
the full table is printed either way, because a pair that is unused today is
worth seeing before someone uses it tomorrow.
"""

import math
import re
import sys
from pathlib import Path

CSS = Path(__file__).resolve().parent.parent / "tauri-rs" / "src" / "styles.css"

AA = 4.5

SURFACES = ["surface-0", "surface-1", "surface-2", "surface-3"]
FOREGROUNDS = ["ink", "ink-dim", "ink-faint", "accent", "ok", "warn", "danger"]

# Which surfaces each foreground is actually drawn on as text. `edge` is absent
# because it only ever draws borders, where AA does not apply; the accent tones
# are absent from surface-3 because nothing puts them there.
USED_ON = {
    "ink": SURFACES,
    "ink-dim": SURFACES,
    # Chips and hovered rows reach surface-2; surface-3 chips use ink-dim.
    "ink-faint": ["surface-0", "surface-1", "surface-2"],
    "accent": ["surface-0", "surface-1", "surface-2"],
    "ok": ["surface-0", "surface-1", "surface-2"],
    "warn": ["surface-0", "surface-1", "surface-2"],
    "danger": ["surface-0", "surface-1", "surface-2"],
}

TOKEN = re.compile(r"--color-([a-z0-9-]+):\s*oklch\(([\d.]+)\s+([\d.]+)\s+([\d.]+)\)")


def oklch_to_linear_srgb(lightness: float, chroma: float, hue_deg: float):
    """OKLCH → linear sRGB. Out-of-gamut components are clamped when used."""
    hue = math.radians(hue_deg)
    a, b = chroma * math.cos(hue), chroma * math.sin(hue)
    l_ = lightness + 0.3963377774 * a + 0.2158037573 * b
    m_ = lightness - 0.1055613458 * a - 0.0638541728 * b
    s_ = lightness - 0.0894841775 * a - 1.2914855480 * b
    l, m, s = l_**3, m_**3, s_**3
    return (
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )


def luminance(linear_rgb) -> float:
    r, g, b = (min(1.0, max(0.0, c)) for c in linear_rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(fg, bg) -> float:
    lo, hi = sorted((luminance(fg), luminance(bg)))
    return (hi + 0.05) / (lo + 0.05)


def themes(css: str):
    """The `@theme` block is dark; the `[data-theme="light"]` block is light."""
    light_at = css.index(':root[data-theme="light"]')
    parse = lambda block: {  # noqa: E731 - a name would not make this clearer
        name: oklch_to_linear_srgb(float(lightness), float(chroma), float(hue))
        for name, lightness, chroma, hue in TOKEN.findall(block)
    }
    return {
        "dark": parse(css[css.index("@theme {") : light_at]),
        "light": parse(css[light_at:]),
    }


def main() -> int:
    strict = "--strict" in sys.argv
    failures = []

    for theme, tokens in themes(CSS.read_text()).items():
        print(f"\n=== {theme} ===")
        print("fg \\ bg".ljust(12) + "".join(s.ljust(10) for s in SURFACES))
        for fg in FOREGROUNDS:
            row = fg.ljust(12)
            for bg in SURFACES:
                ratio = contrast(tokens[fg], tokens[bg])
                used = bg in USED_ON[fg]
                if used and ratio < AA:
                    failures.append(f"{theme}: {fg} on {bg} is {ratio:.2f}:1")
                # "·" marks a pair the UI never renders, so a low number there
                # is a note for the future rather than a defect today.
                mark = "" if used else "·"
                row += f"{ratio:5.2f}{mark}".ljust(10)
            print(row)
        worst = min(
            (contrast(tokens[fg], tokens[bg]), f"{fg} on {bg}")
            for fg in FOREGROUNDS
            for bg in USED_ON[fg]
        )
        print(f"worst rendered pair: {worst[0]:.2f}:1 ({worst[1]})")

    print("\n· = combination the UI does not render")
    if failures:
        print(f"\n{len(failures)} pair(s) below AA {AA}:1:")
        for f in failures:
            print(f"  {f}")
        return 1 if strict else 0
    print(f"\nevery rendered pair clears AA {AA}:1")
    return 0


if __name__ == "__main__":
    sys.exit(main())
