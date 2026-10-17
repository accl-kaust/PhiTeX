#!/usr/bin/env python3
"""An xref-stream-like input (N entries of type, 3-byte offset, index,
offsets rising by 2,000 to 3,000 bytes) written to FILE, and real zlib's
level-9 time on it: `xref-zlib.py FILE [N]` (deflate_time compares)."""
import random, sys, time, zlib
n = int(sys.argv[2]) if len(sys.argv) > 2 else 22285
r = random.Random(1)
off, b = 15, bytearray()
for k in range(n):
    if k % 9 == 0:
        b += bytes([2]) + (k // 9).to_bytes(3, "big") + bytes([k % 200])
    else:
        b += bytes([1]) + off.to_bytes(3, "big") + bytes([0])
        off += r.randint(2000, 3000)
open(sys.argv[1], "wb").write(b)
best = 1e9
for _ in range(5):
    t = time.perf_counter(); z = zlib.compress(bytes(b), 9); best = min(best, time.perf_counter() - t)
c = zlib.compressobj(9, zlib.DEFLATED, -15)
raw = c.compress(bytes(b)) + c.flush()
print(f"zlib: {len(b)} bytes in, {len(z)} out (raw {len(raw)}), adler {zlib.adler32(raw) & 0xffffffff:08x}, best of 5 {best*1e3:.2f} ms")
