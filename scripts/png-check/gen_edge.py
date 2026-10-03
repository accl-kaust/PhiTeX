"""Hand-made PNG edge cases for comparing libpng with partex's port.

    gen_edge.py DIR     writes DIR/*.png
"""
import os
import random
import struct
import sys
import zlib

OUT = sys.argv[1] if len(sys.argv) > 1 else "edge"
os.makedirs(OUT, exist_ok=True)
SIG = b"\x89PNG\r\n\x1a\n"


def chunk(name, data, bad_crc=False):
    crc = zlib.crc32(name + data) & 0xFFFFFFFF
    if bad_crc:
        crc ^= 1
    return struct.pack(">I", len(data)) + name + data + struct.pack(">I", crc)


def ihdr(w, h, depth, color, interlace=0, comp=0, filt=0):
    return chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, color, comp, filt, interlace))


def iend():
    return chunk(b"IEND", b"")


def write(name, *parts):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(SIG + b"".join(parts))


def raw_rows(w, h, bpp_bits, seed=1, filters=(0,)):
    """Filtered rows of random data (each row a filter byte from `filters`)."""
    rnd = random.Random(seed)
    n = (w * bpp_bits + 7) // 8
    out = bytearray()
    for y in range(h):
        out.append(filters[y % len(filters)])
        out += bytes(rnd.randrange(256) for _ in range(n))
    return bytes(out)


def idat(raw, level=6, **kw):
    return chunk(b"IDAT", zlib.compress(raw, level), **kw)


def gray8(w=4, h=4, seed=1):
    return raw_rows(w, h, 8, seed)


def simple(name, *extra, w=4, h=4, depth=8, color=0, raw=None, interlace=0):
    bits = depth * {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
    if raw is None:
        raw = raw_rows(w, h, bits)
    write(name, ihdr(w, h, depth, color, interlace), *extra, idat(raw), iend())


PAL4 = chunk(b"PLTE", bytes([10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]))

# --- ancillary chunks with a bad CRC
simple("crc_gama.png", chunk(b"gAMA", struct.pack(">I", 45455), bad_crc=True))
simple("crc_srgb.png", chunk(b"sRGB", b"\0", bad_crc=True))
simple("crc_trns_gray.png", chunk(b"tRNS", b"\0\x05", bad_crc=True))
simple("crc_trns_pal.png", PAL4, chunk(b"tRNS", b"\0\x80", bad_crc=True), depth=2, color=3)
simple("crc_chrm.png", chunk(b"cHRM", bytes(32), bad_crc=True))
simple("crc_phys.png", chunk(b"pHYs", struct.pack(">IIB", 2835, 2835, 1), bad_crc=True))
simple("crc_bkgd.png", chunk(b"bKGD", b"\0\x01", bad_crc=True))
simple("crc_sbit.png", chunk(b"sBIT", b"\x08", bad_crc=True))
simple("crc_splt.png", chunk(b"sPLT", b"p\0\x08" + bytes(12), bad_crc=True))
simple("crc_text.png", chunk(b"tEXt", b"Title\0x", bad_crc=True))
simple("crc_plte_rgb.png", chunk(b"PLTE", bytes(9), bad_crc=True), color=2)
simple("crc_plte_pal.png", chunk(b"PLTE", bytes(12), bad_crc=True), depth=2, color=3)
simple("crc_ihdr.png")
with open(os.path.join(OUT, "crc_ihdr.png"), "r+b") as f:
    f.seek(8 + 8 + 13)
    b = f.read(1)
    f.seek(8 + 8 + 13)
    f.write(bytes([b[0] ^ 1]))
simple("crc_unknown_anc.png", chunk(b"zzZz", b"abc", bad_crc=True))
simple("crc_unknown_crit.png", chunk(b"ZZZZ", b"abc", bad_crc=True))
simple("unknown_crit.png", chunk(b"ZZZZ", b"abc"))
simple("unknown_anc.png", chunk(b"zzZz", b"abc"), chunk(b"acTL", bytes(8)))

# --- tRNS
simple("trns_short.png", PAL4, chunk(b"tRNS", b"\0\x80"), depth=2, color=3)
simple("trns_full.png", PAL4, chunk(b"tRNS", b"\0\x40\x80\xff"), depth=2, color=3)
simple("trns_long.png", PAL4, chunk(b"tRNS", bytes(5)), depth=2, color=3)
simple("trns_zero.png", PAL4, chunk(b"tRNS", b""), depth=2, color=3)
simple("trns_before_plte.png", chunk(b"tRNS", b"\0"), PAL4, depth=2, color=3)
simple("trns_before_plte_rgb.png", chunk(b"tRNS", bytes(6)), chunk(b"PLTE", bytes(9)), color=2)
simple("bkgd_before_plte_rgb.png", chunk(b"bKGD", bytes(6)), chunk(b"PLTE", bytes(9)), color=2)
simple("trns_rgba.png", chunk(b"tRNS", bytes(6)), color=6)
simple("trns_dup.png", chunk(b"tRNS", b"\0\x01"), chunk(b"tRNS", b"\0\x02"))
simple("trns_gray_bad_len.png", chunk(b"tRNS", b"\0\x01\0"))
simple("trns_after_crc_error.png", chunk(b"tRNS", b"\0\x09", bad_crc=True), chunk(b"tRNS", b"\0\x02"))
# a transparent gray past the depth (masked by png_do_expand)
simple("trns_gray2_range.png", chunk(b"tRNS", b"\0\x06"), depth=2, raw=raw_rows(16, 4, 2, 3))
simple("trns_gray1.png", chunk(b"tRNS", b"\0\x01"), w=13, depth=1)
simple("trns_gray4.png", chunk(b"tRNS", b"\0\x07"), w=5, depth=4)
simple("trns_gray8_wide.png", chunk(b"tRNS", b"\x12\x34"), raw=bytes([0, 0x34, 0x12, 0x34, 0]) * 4)
simple("trns_gray16.png", chunk(b"tRNS", b"\x12\x34"), depth=16,
       raw=bytes([0, 0x12, 0x34, 0x34, 0x12, 0, 0, 0x12, 0x34]) * 4, w=4)
simple("trns_rgb8.png", chunk(b"tRNS", b"\x01\x0a\0\x14\0\x1e"), color=2,
       raw=bytes([0, 10, 20, 30, 10, 20, 31, 0, 0, 0, 10, 20, 30]) * 4)
simple("trns_rgb16.png", chunk(b"tRNS", b"\0\x0a\0\x14\0\x1e"), color=2, depth=16,
       raw=bytes([0, 0, 10, 0, 20, 0, 30, 0, 10, 0, 20, 0, 31]) * 4, w=2)
simple("trns_pal_interlaced.png", PAL4, chunk(b"tRNS", b"\x10\x20\x30"), depth=2, color=3,
       w=13, h=11, interlace=1)
# palette indices past the palette (black; opaque past tRNS)
simple("pal_out_of_range.png", chunk(b"PLTE", bytes(range(6))), w=8, h=2, depth=8, color=3,
       raw=bytes([0, 0, 1, 2, 3, 200, 255, 1, 0]) * 2)
simple("pal_out_of_range_trns.png", chunk(b"PLTE", bytes(range(6))), chunk(b"tRNS", b"\x07"),
       w=8, h=2, depth=8, color=3, raw=bytes([0, 0, 1, 2, 3, 200, 255, 1, 0]) * 2)

# --- iCCP
def iccp(profile, keyword=b"ICC", method=0, level=9, extra=b"", bad_crc=False, cut=None):
    z = zlib.compress(profile, level) + extra
    if cut is not None:
        z = z[:cut]
    return chunk(b"iCCP", keyword + b"\0" + bytes([method]) + z, bad_crc=bad_crc)


def readf(p):
    with open(p, "rb") as f:
        return f.read()


SRGB = readf("/usr/share/color/icc/colord/sRGB.icc")
GS_SRGB = readf("/usr/share/ghostscript/iccprofiles/srgb.icc")
GRAY = readf("/usr/share/ghostscript/iccprofiles/default_gray.icc")
SGRAY = readf("/usr/share/ghostscript/iccprofiles/sgray.icc")
LAB = readf("/usr/share/ghostscript/iccprofiles/lab.icc")
CMYK = readf("/usr/share/ghostscript/iccprofiles/default_cmyk.icc")
simple("iccp_srgb.png", iccp(SRGB), color=2)
simple("iccp_gs_srgb.png", iccp(GS_SRGB), color=2)
simple("iccp_srgb_pal.png", iccp(GS_SRGB), PAL4, depth=2, color=3)
simple("iccp_gray_on_rgb.png", iccp(GRAY), color=2)
simple("iccp_rgb_on_gray.png", iccp(GS_SRGB))
simple("iccp_gray.png", iccp(GRAY))
simple("iccp_sgray.png", iccp(SGRAY), color=4)
simple("iccp_lab.png", iccp(LAB), color=2)
simple("iccp_cmyk.png", iccp(CMYK), color=2)
simple("iccp_crc.png", iccp(GS_SRGB, bad_crc=True), color=2)
simple("iccp_extra.png", iccp(GS_SRGB, extra=b"junkjunk"), color=2)
simple("iccp_truncated.png", iccp(GS_SRGB, cut=600), color=2)
simple("iccp_method1.png", iccp(GS_SRGB, method=1), color=2)
simple("iccp_nokeyword.png", iccp(GS_SRGB, keyword=b""), color=2)
simple("iccp_longkeyword.png", iccp(GS_SRGB, keyword=b"k" * 80), color=2)
simple("iccp_kw79.png", iccp(GS_SRGB, keyword=b"k" * 79), color=2)
simple("iccp_and_srgb.png", chunk(b"sRGB", b"\0"), iccp(GS_SRGB), color=2)
simple("iccp_dup.png", iccp(GS_SRGB), iccp(GS_SRGB), color=2)
simple("iccp_after_plte.png", chunk(b"PLTE", bytes(9)), iccp(GS_SRGB), color=2)
bad_adler = bytearray(zlib.compress(GS_SRGB, 9))
bad_adler[-1] ^= 1
simple("iccp_bad_adler.png", chunk(b"iCCP", b"ICC\0\0" + bytes(bad_adler)), color=2)
simple("iccp_short_chunk.png", chunk(b"iCCP", b"a\0\0" + zlib.compress(b"x" * 10)), color=2)
p = bytearray(GS_SRGB)
p[0:4] = struct.pack(">I", len(p) + 4)
simple("iccp_badlen.png", iccp(bytes(p)), color=2)
p = bytearray(GS_SRGB)
p[128:132] = struct.pack(">I", 10000)
simple("iccp_tagcount.png", iccp(bytes(p)), color=2)
p = bytearray(GS_SRGB)
p[132 + 4:132 + 8] = struct.pack(">I", len(p) - 2)
simple("iccp_tag_outside.png", iccp(bytes(p)), color=2)
p = bytearray(GS_SRGB)
p[12:16] = b"link"
simple("iccp_link.png", iccp(bytes(p)), color=2)
p = bytearray(GS_SRGB)
p[36:40] = b"acsq"
simple("iccp_sig.png", iccp(bytes(p)), color=2)
p = bytearray(132)
p[0:4] = struct.pack(">I", 132)
p[12:24] = b"mntrRGB XYZ "
p[36:40] = b"acsp"
simple("iccp_header_only.png", iccp(bytes(p)), color=2)
simple("iccp_stored.png", iccp(GS_SRGB, level=0), color=2)

# --- IDAT layout
RAW = raw_rows(37, 23, 24, 7, filters=(0, 1, 2, 3, 4))
Z = zlib.compress(RAW, 9)


def split_idats(z, sizes):
    out, i, k = [], 0, 0
    while i < len(z):
        n = sizes[k % len(sizes)]
        out.append(chunk(b"IDAT", z[i:i + n]))
        i += n
        k += 1
    return out


write("idat_one.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z), iend())
write("idat_split1.png", ihdr(37, 23, 8, 2), *split_idats(Z, [1]), iend())
write("idat_split_odd.png", ihdr(37, 23, 8, 2), *split_idats(Z, [3, 1, 7, 100, 2]), iend())
write("idat_zero_between.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:100]), chunk(b"IDAT", b""),
      chunk(b"IDAT", Z[100:]), iend())
write("idat_zero_first.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", b""), chunk(b"IDAT", Z), iend())
write("idat_zero_after.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z), chunk(b"IDAT", b"", bad_crc=True),
      iend())
write("idat_garbage_after.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z),
      chunk(b"IDAT", b"garbage", bad_crc=True), iend())
write("idat_crc_first.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:50], bad_crc=True),
      chunk(b"IDAT", Z[50:]), iend())
write("idat_crc_last.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:50]),
      chunk(b"IDAT", Z[50:], bad_crc=True), iend())
write("idat_crc_only.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z, bad_crc=True), iend())
write("idat_text_between.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:50]), chunk(b"tEXt", b"a\0b"),
      chunk(b"IDAT", Z[50:]), iend())
write("idat_trailer_apart.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:-4]), chunk(b"IDAT", Z[-4:]),
      iend())
write("idat_trailer_missing.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:-4]), iend())
write("idat_trailer_missing_eof.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z[:-4]))
write("idat_no_iend.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z))
write("idat_cut.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z)[:200])
write("idat_cut_crc.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z)[:-2])
write("idat_extra_bytes.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", Z + b"extra"), iend())
write("idat_too_much.png", ihdr(37, 22, 8, 2), chunk(b"IDAT", Z), iend())
write("idat_too_little.png", ihdr(37, 24, 8, 2), chunk(b"IDAT", Z), iend())
write("idat_no_plte.png", ihdr(4, 4, 8, 3), idat(gray8()), iend())
write("idat_bad_filter.png", ihdr(4, 4, 8, 0), idat(gray8()[:-5] + b"\x05" + gray8()[-4:]), iend())
z = bytearray(Z)
z[-1] ^= 1
write("z_bad_adler.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", bytes(z)), iend())
write("z_bad_adler_apart.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", bytes(z[:-4])),
      chunk(b"IDAT", bytes(z[-4:])), iend())
write("z_bad_adler_last_byte_apart.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", bytes(z[:-1])),
      chunk(b"IDAT", bytes(z[-1:])), iend())

# stored blocks laid out so the image data ends exactly at the end of the
# first 8,192-byte read: the trailer is read only after the last row
W, H = 1636, 5
raw = raw_rows(W, H, 8, 9)
assert 2 + 5 + len(raw) == 8192


def stored(data, adler_xor=0, last=True):
    out = b"\x78\x01" + bytes([1 if last else 0]) + struct.pack("<HH", len(data), len(data) ^ 0xFFFF)
    return out + data + struct.pack(">I", zlib.adler32(data) ^ adler_xor)


write("z_stored_boundary.png", ihdr(W, H, 8, 0), chunk(b"IDAT", stored(raw)), iend())
write("z_stored_boundary_bad_adler.png", ihdr(W, H, 8, 0), chunk(b"IDAT", stored(raw, 1)), iend())
write("z_stored_boundary_no_adler.png", ihdr(W, H, 8, 0), chunk(b"IDAT", stored(raw)[:-4]), iend())
raw2 = raw_rows(W, H, 8, 9)[:-1] + b"\x00"
write("z_stored_bad_adler.png", ihdr(W - 1, H, 8, 0),
      chunk(b"IDAT", stored(raw_rows(W - 1, H, 8, 9), 1)), iend())
# extra stored data after the image (benign: too much image data)
write("z_stored_extra.png", ihdr(W, H - 1, 8, 0), chunk(b"IDAT", stored(raw)), iend())
# garbage (an invalid block type) right after the last block's data
def fixed_then(data, tail_bits):
    co = zlib.compressobj(9, zlib.DEFLATED, -15, 9, zlib.Z_FIXED)
    body = co.compress(data) + co.flush(zlib.Z_SYNC_FLUSH)
    return b"\x78\x01" + body + tail_bits


small = raw_rows(5, 3, 8, 4)
write("z_bad_block_after.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", fixed_then(small, b"\x07\xff\xff")), iend())
write("z_bad_block_apart.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", fixed_then(small, b"")),
      chunk(b"IDAT", b"\x07\xff\xff"), iend())
write("z_sync_no_end.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", fixed_then(small, b"")), iend())
# zlib headers
def with_header(h):
    return h + zlib.compress(small)[2:]


write("z_fdict.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", b"\x78\xbb" + b"\0\0\0\1" + zlib.compress(small)[2:]), iend())
write("z_fcheck.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", with_header(b"\x78\x9d")), iend())
write("z_cinfo8.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", with_header(b"\x88\x1c")), iend())
write("z_method7.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", with_header(b"\x77\x09")), iend())
write("z_cinfo1.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", with_header(b"\x18\x19")), iend())
for lvl in (0, 1, 9):
    write(f"z_level{lvl}.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", zlib.compress(RAW, lvl)), iend())
co = zlib.compressobj(9, zlib.DEFLATED, 15, 9, zlib.Z_FIXED)
write("z_fixed.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", co.compress(RAW) + co.flush()), iend())
co = zlib.compressobj(9, zlib.DEFLATED, 15, 9, zlib.Z_HUFFMAN_ONLY)
write("z_huffman.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", co.compress(RAW) + co.flush()), iend())
co = zlib.compressobj(9, zlib.DEFLATED, 15, 9, zlib.Z_RLE)
write("z_rle.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", co.compress(RAW) + co.flush()), iend())
# a distance too far back: a fixed block "copy 3 bytes from 1 back" first
write("z_too_far.png", ihdr(5, 3, 8, 0), chunk(b"IDAT", b"\x78\x01" + bytes([0x03, 0x02, 0x00])), iend())
# random damage
rnd = random.Random(5)
for k in range(40):
    z = bytearray(zlib.compress(RAW, 6))
    for _ in range(1 + k % 3):
        i = rnd.randrange(2, len(z))
        z[i] ^= 1 << rnd.randrange(8)
    write(f"z_damaged{k:02}.png", ihdr(37, 23, 8, 2), chunk(b"IDAT", bytes(z)), iend())

# a large noisy image across many reads and chunks
BIG = raw_rows(301, 157, 24, 11, filters=(0, 1, 2, 3, 4))
ZB = zlib.compress(BIG, 6)
write("big.png", ihdr(301, 157, 8, 2), chunk(b"IDAT", ZB), iend())
write("big_split.png", ihdr(301, 157, 8, 2), *split_idats(ZB, [8191, 1, 9000, 4096]), iend())
zb = bytearray(ZB)
zb[-2] ^= 4
write("big_bad_adler.png", ihdr(301, 157, 8, 2), chunk(b"IDAT", bytes(zb)), iend())
BIGI = raw_rows(301, 157, 24, 11)  # not an interlaced layout: lengths differ, still a test
write("big_interlaced_wrong.png", ihdr(301, 157, 8, 2, 1), chunk(b"IDAT", zlib.compress(BIGI)), iend())


def adam7_raw(w, h, bits, seed, filters=(0, 1, 2, 3, 4)):
    rnd = random.Random(seed)
    out = bytearray()
    k = 0
    for xs, xi, ys, yi in ((0, 8, 0, 8), (4, 8, 0, 8), (0, 4, 4, 8), (2, 4, 0, 4), (0, 2, 2, 4),
                           (1, 2, 0, 2), (0, 1, 1, 2)):
        pw = (w - xs + xi - 1) // xi if w > xs else 0
        if pw == 0:
            continue
        for y in range(ys, h, yi):
            out.append(filters[k % len(filters)])
            k += 1
            out += bytes(rnd.randrange(256) for _ in range((pw * bits + 7) // 8))
    return bytes(out)


for (w, h, depth, color) in ((301, 157, 8, 2), (13, 11, 1, 0), (9, 7, 2, 0), (5, 3, 4, 3), (17, 9, 16, 6),
                             (1, 1, 8, 0), (2, 9, 16, 0), (9, 2, 8, 4), (7, 7, 16, 2)):
    bits = depth * {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
    extra = [chunk(b"PLTE", bytes(range(48)))] if color == 3 else []
    write(f"adam7_{w}x{h}_{depth}_{color}.png", ihdr(w, h, depth, color, 1), *extra,
          chunk(b"IDAT", zlib.compress(adam7_raw(w, h, bits, w * h))), iend())

# --- truncation
full = SIG + ihdr(37, 23, 8, 2) + chunk(b"gAMA", struct.pack(">I", 45455)) + chunk(b"IDAT", Z) + iend()
for n in (0, 5, 8, 12, 20, 33, 40, 49, 53, 60, 61, 100, len(full) - 12, len(full) - 5):
    with open(os.path.join(OUT, f"trunc_{n:04}.png"), "wb") as f:
        f.write(full[:n])

# --- IHDR
write("ihdr_w0.png", ihdr(0, 4, 8, 0), idat(gray8()), iend())
write("ihdr_h_big.png", ihdr(4, 1000001, 8, 0), idat(gray8()), iend())
write("ihdr_w_max.png", ihdr(1000000, 1, 1, 0), idat(b"\0" + bytes(125000)), iend())
write("ihdr_w_31.png", ihdr(0x80000000, 1, 1, 0), idat(gray8()), iend())
write("ihdr_depth3.png", ihdr(4, 4, 3, 0), idat(gray8()), iend())
write("ihdr_color1.png", ihdr(4, 4, 8, 1), idat(gray8()), iend())
write("ihdr_color5.png", ihdr(4, 4, 8, 5), idat(gray8()), iend())
write("ihdr_color7.png", ihdr(4, 4, 8, 7), idat(gray8()), iend())
write("ihdr_pal16.png", ihdr(4, 4, 16, 3), PAL4, idat(gray8()), iend())
write("ihdr_rgb4.png", ihdr(4, 4, 4, 2), idat(gray8()), iend())
write("ihdr_interlace2.png", ihdr(4, 4, 8, 0, 2), idat(gray8()), iend())
write("ihdr_comp1.png", ihdr(4, 4, 8, 0, 0, 1), idat(gray8()), iend())
write("ihdr_filter1.png", ihdr(4, 4, 8, 0, 0, 0, 1), idat(gray8()), iend())
write("ihdr_filter64.png", ihdr(4, 4, 8, 2, 0, 0, 64), idat(gray8()), iend())
write("ihdr_len12.png", chunk(b"IHDR", struct.pack(">IIBBBB", 4, 4, 8, 0, 0, 0)), idat(gray8()), iend())
write("ihdr_len14.png", chunk(b"IHDR", struct.pack(">IIBBBBBB", 4, 4, 8, 0, 0, 0, 0, 0)), idat(gray8()),
      iend())
write("ihdr_twice.png", ihdr(4, 4, 8, 0), ihdr(4, 4, 8, 0), idat(gray8()), iend())
write("ihdr_missing.png", chunk(b"gAMA", struct.pack(">I", 1)), ihdr(4, 4, 8, 0), idat(gray8()), iend())
write("ihdr_after_unknown.png", chunk(b"zzZz", b""), ihdr(4, 4, 8, 0), idat(gray8()), iend())
write("ihdr_none.png", idat(gray8()), iend())
write("ihdr_none_plte.png", PAL4, ihdr(4, 4, 8, 0), idat(gray8()), iend())
write("iend_before_idat.png", ihdr(4, 4, 8, 0), iend(), idat(gray8()), iend())
write("no_idat.png", ihdr(4, 4, 8, 0), iend())
write("bad_name.png", ihdr(4, 4, 8, 0), chunk(b"zzzz", b""), idat(gray8()), iend())
write("bad_name2.png", ihdr(4, 4, 8, 0), chunk(b"z1Zz", b""), idat(gray8()), iend())
with open(os.path.join(OUT, "bad_length.png"), "wb") as f:
    f.write(SIG + ihdr(4, 4, 8, 0) + b"\x80\0\0\0tEXt" + b"\0" * 20)
with open(os.path.join(OUT, "bad_sig.png"), "wb") as f:
    f.write(b"\x89PNG\n\x1a\n\0" + ihdr(4, 4, 8, 0)[:-1])
with open(os.path.join(OUT, "bad_sig2.png"), "wb") as f:
    f.write(b"GIF89a\0\0" + ihdr(4, 4, 8, 0))

# --- PLTE
simple("plte_zero.png", chunk(b"PLTE", b""), depth=2, color=3)
simple("plte_zero_rgb.png", chunk(b"PLTE", b""), color=2)
simple("plte_gray.png", chunk(b"PLTE", bytes(9)))
simple("plte_dup_pal.png", PAL4, PAL4, depth=2, color=3)
simple("plte_dup_rgb.png", chunk(b"PLTE", bytes(9)), chunk(b"PLTE", bytes(9)), color=2)
simple("plte_long_pal.png", PAL4, depth=1, color=3)
simple("plte_mod3_pal.png", chunk(b"PLTE", bytes(7)), depth=2, color=3)
simple("plte_mod3_rgb.png", chunk(b"PLTE", bytes(7)), color=2)
simple("plte_769.png", chunk(b"PLTE", bytes(771)), depth=8, color=3)
simple("plte_256.png", chunk(b"PLTE", bytes(range(256)) * 3), depth=8, color=3)
simple("plte_rgb.png", chunk(b"PLTE", bytes(range(30))), color=6)

# --- gAMA, cHRM, sRGB, sBIT, bKGD, hIST, pHYs, sPLT
simple("gama_100000.png", chunk(b"gAMA", struct.pack(">I", 100000)))
simple("gama_0.png", chunk(b"gAMA", struct.pack(">I", 0)))
simple("gama_big.png", chunk(b"gAMA", struct.pack(">I", 0x80000000)))
simple("gama_short.png", chunk(b"gAMA", b"\0\0\1"))
simple("gama_dup.png", chunk(b"gAMA", struct.pack(">I", 100000)), chunk(b"gAMA", struct.pack(">I", 2)))
simple("gama_after_plte.png", chunk(b"PLTE", bytes(9)), chunk(b"gAMA", struct.pack(">I", 1)), color=2)
simple("gama_after_crc.png", chunk(b"gAMA", struct.pack(">I", 7), bad_crc=True),
       chunk(b"gAMA", struct.pack(">I", 100000)))
simple("chrm_ok.png", chunk(b"cHRM", struct.pack(">8i", 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)))
simple("chrm_neg.png", chunk(b"cHRM", struct.pack(">8i", -1, 2, 3, 4, 5, 6, 7, 8)))
simple("chrm_min.png", chunk(b"cHRM", struct.pack(">8I", 0x80000000, 2, 3, 4, 5, 6, 7, 8)))
simple("srgb_4.png", chunk(b"sRGB", b"\x04"))
simple("srgb_3.png", chunk(b"sRGB", b"\x03"))
simple("sbit_ok.png", chunk(b"sBIT", b"\x05\x06\x07"), color=2)
simple("sbit_zero.png", chunk(b"sBIT", b"\x05\x00\x07"), color=2)
simple("sbit_big.png", chunk(b"sBIT", b"\x09"))
simple("sbit_len.png", chunk(b"sBIT", b"\x05\x05"))
simple("sbit_pal.png", PAL4, depth=2, color=3)
simple("sbit_pal2.png", chunk(b"sBIT", b"\x08\x08\x08"), PAL4, depth=2, color=3)
simple("sbit_ga.png", chunk(b"sBIT", b"\x08\x01"), color=4)
simple("bkgd_gray_ok.png", chunk(b"bKGD", b"\0\x03"), depth=2)
simple("bkgd_gray_range.png", chunk(b"bKGD", b"\0\x04"), depth=2)
simple("bkgd_gray16.png", chunk(b"bKGD", b"\xff\xff"), depth=16)
simple("bkgd_rgb_hi.png", chunk(b"bKGD", b"\x01\0\0\0\0\0"), color=2)
simple("bkgd_pal_ok.png", PAL4, chunk(b"bKGD", b"\x03"), depth=2, color=3)
simple("bkgd_pal_range.png", PAL4, chunk(b"bKGD", b"\x04"), depth=2, color=3)
simple("bkgd_pal_noplte.png", chunk(b"bKGD", b"\x00"), PAL4, depth=2, color=3)
simple("hist_before.png", chunk(b"hIST", b""), PAL4, depth=2, color=3)
simple("hist_after.png", PAL4, chunk(b"hIST", bytes(8)), depth=2, color=3)
simple("phys_0.png", chunk(b"pHYs", struct.pack(">IIB", 1, 2, 0)))
simple("phys_1.png", chunk(b"pHYs", struct.pack(">IIB", 3780, 3780, 1)))
simple("phys_7.png", chunk(b"pHYs", struct.pack(">IIB", 3780, 37, 7)))
simple("phys_after_plte.png", chunk(b"PLTE", bytes(9)), chunk(b"pHYs", struct.pack(">IIB", 3780, 3780, 1)),
       color=2)
simple("splt_ok.png", chunk(b"sPLT", b"pal\0\x08" + bytes(12)))
simple("splt_16.png", chunk(b"sPLT", b"pal\0\x10" + bytes(20)))
simple("splt_7.png", chunk(b"sPLT", b"pal\0\x07" + bytes(10)))
simple("splt_empty.png", chunk(b"sPLT", b"pal\0\x08"))
simple("splt_badlen.png", chunk(b"sPLT", b"pal\0\x08" + bytes(7)))
simple("splt_noname.png", chunk(b"sPLT", b"palette"))
simple("splt_two.png", chunk(b"sPLT", b"a\0\x08" + bytes(6)), chunk(b"sPLT", b"b\0\x08" + bytes(6)))
simple("splt_cache.png", *[chunk(b"tEXt", b"k\0v")] * 997, chunk(b"sPLT", b"a\0\x08" + bytes(6)))
simple("splt_cache2.png", *[chunk(b"tEXt", b"k\0v")] * 998, chunk(b"sPLT", b"a\0\x08" + bytes(6)))
simple("text_big.png", chunk(b"tEXt", b"k\0" + bytes(100)), chunk(b"zTXt", b"k\0\0" + zlib.compress(b"hi")),
       chunk(b"iTXt", b"k\0\0\0\0\0hello"), chunk(b"zTXt", b"k\0\1junkjunkjunkjunk"))
# the rest of the valid bits
simple("time_ok.png", chunk(b"tIME", struct.pack(">HBBBBB", 2024, 1, 2, 3, 4, 5)))
simple("time_bad.png", chunk(b"tIME", struct.pack(">HBBBBB", 2024, 13, 2, 3, 4, 5)))
simple("time_dup.png", chunk(b"tIME", struct.pack(">HBBBBB", 2024, 13, 2, 3, 4, 5)),
       chunk(b"tIME", struct.pack(">HBBBBB", 2024, 1, 2, 3, 4, 5)))
simple("offs.png", chunk(b"oFFs", struct.pack(">iiB", -5, 7, 1)))
simple("pcal_ok.png", chunk(b"pCAL", b"temp\0" + struct.pack(">ii", 0, 255) + b"\x00\x02K\x001.5\x002e3"))
simple("pcal_badparam.png", chunk(b"pCAL", b"temp\0" + struct.pack(">ii", 0, 255) + b"\x00\x02K\x001.5\x00x"))
simple("pcal_count.png", chunk(b"pCAL", b"temp\0" + struct.pack(">ii", 0, 255) + b"\x00\x03K\x001\x002\x003"))
simple("pcal_type9.png", chunk(b"pCAL", b"temp\0" + struct.pack(">ii", 0, 255) + b"\x09\x01K\x001"))
simple("pcal_short.png", chunk(b"pCAL", b"temp\0" + struct.pack(">ii", 0, 255) + b"\x00\x02K\x001"))
simple("scal_ok.png", chunk(b"sCAL", b"\x011.5\x002e-3"))
simple("scal_neg.png", chunk(b"sCAL", b"\x01-1.5\x002"))
simple("scal_zero.png", chunk(b"sCAL", b"\x010\x002"))
simple("scal_unit.png", chunk(b"sCAL", b"\x031\x002"))
simple("scal_trail.png", chunk(b"sCAL", b"\x011\x002x"))
simple("exif_ok.png", chunk(b"eXIf", b"MM\0*\0\0\0\x08"))
simple("exif_bad.png", chunk(b"eXIf", b"MX\0*\0\0\0\x08"))
simple("cicp_ok.png", chunk(b"cICP", b"\x01\x0d\x00\x01"))
simple("cicp_matrix.png", chunk(b"cICP", b"\x01\x0d\x01\x01"))
simple("clli_ok.png", chunk(b"cLLI", struct.pack(">II", 1000, 400)))
simple("clli_big.png", chunk(b"cLLI", struct.pack(">II", 0x80000000, 400)))
simple("mdcv_ok.png", chunk(b"mDCV", struct.pack(">8HII", *range(8), 100, 1)))
simple("mdcv_big.png", chunk(b"mDCV", struct.pack(">8HII", *range(8), 0x80000000, 1)))
simple("everything.png", chunk(b"gAMA", struct.pack(">I", 45455)),
       chunk(b"cHRM", struct.pack(">8i", 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)),
       chunk(b"sRGB", b"\0"), chunk(b"sBIT", b"\x08\x08\x08"), iccp(GS_SRGB), chunk(b"PLTE", bytes(9)),
       chunk(b"bKGD", bytes(6)), chunk(b"pHYs", struct.pack(">IIB", 3780, 3780, 1)),
       chunk(b"sPLT", b"a\0\x08" + bytes(6)), chunk(b"tRNS", bytes(6)),
       chunk(b"tIME", struct.pack(">HBBBBB", 2024, 1, 2, 3, 4, 5)), color=2)
print(len(os.listdir(OUT)), "files")
