#!/usr/bin/env python3
"""Reads otf-math's output and prints the same lines from fontTools' MATH
parsing (the reference for partex-otf's math.rs)."""
import sys
from fontTools.ttLib import TTFont

CONST = ["ScriptPercentScaleDown", "ScriptScriptPercentScaleDown", "DelimitedSubFormulaMinHeight",
 "DisplayOperatorMinHeight", "MathLeading", "AxisHeight", "AccentBaseHeight", "FlattenedAccentBaseHeight",
 "SubscriptShiftDown", "SubscriptTopMax", "SubscriptBaselineDropMin", "SuperscriptShiftUp",
 "SuperscriptShiftUpCramped", "SuperscriptBottomMin", "SuperscriptBaselineDropMax", "SubSuperscriptGapMin",
 "SuperscriptBottomMaxWithSubscript", "SpaceAfterScript", "UpperLimitGapMin", "UpperLimitBaselineRiseMin",
 "LowerLimitGapMin", "LowerLimitBaselineDropMin", "StackTopShiftUp", "StackTopDisplayStyleShiftUp",
 "StackBottomShiftDown", "StackBottomDisplayStyleShiftDown", "StackGapMin", "StackDisplayStyleGapMin",
 "StretchStackTopShiftUp", "StretchStackBottomShiftDown", "StretchStackGapAboveMin", "StretchStackGapBelowMin",
 "FractionNumeratorShiftUp", "FractionNumeratorDisplayStyleShiftUp", "FractionDenominatorShiftDown",
 "FractionDenominatorDisplayStyleShiftDown", "FractionNumeratorGapMin", "FractionNumDisplayStyleGapMin",
 "FractionRuleThickness", "FractionDenominatorGapMin", "FractionDenomDisplayStyleGapMin",
 "SkewedFractionHorizontalGap", "SkewedFractionVerticalGap", "OverbarVerticalGap", "OverbarRuleThickness",
 "OverbarExtraAscender", "UnderbarVerticalGap", "UnderbarRuleThickness", "UnderbarExtraDescender",
 "RadicalVerticalGap", "RadicalDisplayStyleVerticalGap", "RadicalRuleThickness", "RadicalExtraAscender",
 "RadicalKernBeforeDegree", "RadicalKernAfterDegree", "RadicalDegreeBottomRaisePercent"]

def val(v):
    return v if isinstance(v, int) else v.Value

for line in sys.stdin:
    if not line.startswith("F "):
        continue
    path = line[2:].strip()
    if path.endswith(".woff2"):
        continue  # fontTools needs the brotli module for these
    f = TTFont(path, lazy=False, fontNumber=0)  # otf-math dumps face 0
    m = f["MATH"].table
    order = f.getGlyphOrder()
    names = {n: i for i, n in enumerate(order)}

    class Gid(dict):
        # fontTools names a glyph id past numGlyphs "glyphNNNNN"
        def __missing__(self, n):
            return int(n[5:])
    gid = Gid(names)
    print("F " + path)
    mc = m.MathConstants
    print("C " + " ".join(str(val(getattr(mc, c))) for c in CONST))
    mv = m.MathVariants
    print("O %d" % (mv.MinConnectorOverlap if mv else 0))
    gi = m.MathGlyphInfo
    def table(t, cov, recs):
        c = getattr(t, cov, None) if t else None
        if not c:
            return {}
        return {gid[g]: r.Value for g, r in zip(c.glyphs, getattr(t, recs))}
    ic = table(gi.MathItalicsCorrectionInfo, "Coverage", "ItalicsCorrection") if gi else {}
    ta = table(gi.MathTopAccentAttachment, "TopAccentCoverage", "TopAccentAttachment") if gi else {}
    kern = {}
    if gi and gi.MathKernInfo and gi.MathKernInfo.MathKernCoverage:
        for g, rec in zip(gi.MathKernInfo.MathKernCoverage.glyphs, gi.MathKernInfo.MathKernInfoRecords):
            kern[gid[g]] = [rec.TopRightMathKern, rec.TopLeftMathKern, rec.BottomRightMathKern, rec.BottomLeftMathKern]
    cons = [{}, {}]
    if mv:
        for k, (cov, lst) in enumerate([(mv.VertGlyphCoverage, mv.VertGlyphConstruction), (mv.HorizGlyphCoverage, mv.HorizGlyphConstruction)]):
            if cov:
                for g, c in zip(cov.glyphs, lst):
                    cons[k][gid[g]] = c
    def kat(k, h):
        if k is None:
            return 0
        hs = [r.Value for r in k.CorrectionHeight]
        i = sum(1 for x in hs if x <= h)
        return k.KernValue[i].Value
    for g in range(len(order)):
        out = "G %d %d %s" % (g, ic.get(g, 0), str(ta[g]) if g in ta else "-")
        for k in range(2):
            c = cons[k].get(g)
            vs = [(gid[r.VariantGlyph], r.AdvanceMeasurement) for r in c.MathGlyphVariantRecord] if c else []
            parts, aic = [], 0
            if c and c.GlyphAssembly:
                a = c.GlyphAssembly
                aic = a.ItalicsCorrection.Value
                parts = [(gid[p.glyph], p.StartConnectorLength, p.EndConnectorLength, p.FullAdvance, bool(p.PartFlags & 1)) for p in a.PartRecords]
            out += " | %s %s %d" % (str(vs), str(parts).replace("True", "true").replace("False", "false"), aic)
        for side in range(4):
            ks = [kat(kern[g][side] if g in kern else None, h) for h in [-1000, -200, 0, 150, 400, 700, 1500]]
            out += " " + str(ks)
        print(out)
