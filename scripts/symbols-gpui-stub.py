#!/usr/bin/env python3
"""Give Material Symbols subsets an empty 'm' glyph.

gpui's Linux text system (cosmic_text_system.rs, load_family) drops any font
whose charmap has no 'm', so an icon-only subset never loads. This adds a blank
'm' glyph (one em wide, no outline) mapped in every Unicode cmap. Icons are
unchanged. Run it after re-subsetting:

    nix-shell -p python3Packages.fonttools --run \
        "python3 scripts/symbols-gpui-stub.py crates/ui-gpui/fonts/MaterialSymbols-*.ttf"
"""
import sys

from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

for path in sys.argv[1:]:
    font = TTFont(path)
    cmap = font.getBestCmap()
    if ord("m") in cmap:
        print(f"{path}: has 'm' already")
        continue
    order = font.getGlyphOrder()
    name = "m"
    font.setGlyphOrder(order + [name])
    font["glyf"].glyphs[name] = TTGlyphPen(None).glyph()
    font["glyf"].glyphOrder = font.getGlyphOrder()
    font["hmtx"].metrics[name] = (font["head"].unitsPerEm, 0)
    for table in font["cmap"].tables:
        if table.isUnicode():
            table.cmap[ord("m")] = name
    font["maxp"].numGlyphs = len(font.getGlyphOrder())
    font.save(path)
    print(f"{path}: added a blank 'm'")
