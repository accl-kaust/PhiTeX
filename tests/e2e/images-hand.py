# images-hand.pdf: a PDF file made by hand, with what pdfTeX's own lack (a
# classic xref table, attributes and resources inherited from the page
# tree and merged, a contents array, a named destination, strings and
# names with escapes):  python3 images-hand.py
import zlib
objs = {}
c1 = b"q 1 0 0 RG 2 w 10 10 m 150 80 l S Q"
c2 = b"q 0 0 1 rg 20 20 30 30 re f Q\n"
objs[1] = b"<< /Type /Catalog /Pages 2 0 R /Dests << /fig2 [5 0 R /Fit] >> >>"
objs[2] = (b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200.5 100.25]"
           b" /Resources << /ProcSet [/PDF /Text] /ExtGState << /G#20one 9 0 R >> /XObject << /X1 10 0 R >> >> /Rotate 270 >>")
objs[3] = b"<< /Type /Page /Parent 2 0 R /Contents [6 0 R 7 0 R] /CropBox [5.5 4.25 190 -3] >>"
objs[4] = (b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Rotate -90 /ArtBox [1 2 3 4] /BleedBox [0 0 50 50]"
           b" /TrimBox [10 10 400 400] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /Shading 8 0 R >>"
           b" /PieceInfo << /App << /Private (keep \\(me\\) \\\\ \\n tab\\t \\377) >> >> /LastModified (D:20250101) >>")
objs[5] = b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Rotate 180 /Group << /S /Transparency >> /Metadata 11 0 R >>"
objs[6] = (b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(zlib.compress(c1))) + zlib.compress(c1) + b"\nendstream"
objs[7] = (b"<< /Length %d >>\nstream\n" % len(c2)) + c2 + b"endstream"
objs[8] = b"<< /Sh1 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >>"
objs[9] = b"<< /Type /ExtGState /CA 0.5 /ca .25 /LW -1.5 /D [[3 1.5] 0] /Name (a\x00b) /Key <48656c6c6f> /Nil null /T true /F false >>"
xf = b"0 0 m 5 5 l S"
objs[10] = (b"<< /Type /XObject /Subtype /Form /BBox [0 0 5 5] /Resources << /ExtGState << /G#20one 9 0 R >> >> /Length %d >>\nstream\n" % len(xf)) + xf + b"\nendstream"
md = b"<?xpacket?><x:xmpmeta/>"
objs[11] = (b"<< /Type /Metadata /Subtype /XML /Length %d >>\nstream\n" % len(md)) + md + b"\nendstream"
objs[12] = b"<< /Producer (by hand) /Title (Hand \\(made\\)) >>"
out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
off = {}
for n in sorted(objs):
    off[n] = len(out)
    out += b"%d 0 obj\n" % n + objs[n] + b"\nendobj\n"
x = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (max(objs) + 1)
for n in range(1, max(objs) + 1):
    out += b"%010d 00000 n \n" % off[n]
out += b"trailer\n<< /Size %d /Root 1 0 R /Info 12 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (max(objs) + 1, x)
open("images-hand.pdf", "wb").write(out)
