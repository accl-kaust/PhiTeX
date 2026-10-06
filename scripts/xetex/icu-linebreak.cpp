// ICU's line break rules, compiled, as XeTeX's \XeTeXlinebreaklocale uses
// them (`ubrk_open(UBRK_LINE, locale)`): read from the ICU XeTeX runs with
// (`ubrk_getBinaryRules`) and written small for
// crates/partex-core/src/icu_linebreak.rs:
//
//   g++ -O1 -o target/x/icu-linebreak scripts/xetex/icu-linebreak.cpp -licuuc
//   scripts/sandbox target/x/icu-linebreak crates/partex-core/src/icu_linebreak.bin
//
// The rule sets, in this order, by the locale that opens each:
//   line (en), line_loose (en@lb=loose), line_normal (en@lb=normal),
//   line_cj (zh), line_loose_cj (zh@lb=loose), line_normal_cj (zh@lb=normal).
// Several share a forward state table (they differ in their character
// classes only); each table is written once.
//
// Format, little-endian:
//   "LB" u8 icu-major u8 0
//   u8 tables; each: u8 cats, u16 states, u8 dict_start, u8 lookahead, u8 flags,
//     then per state: u16 base (0xFFFF: none, all 0), u8 n, n x (u8 col, u16 value):
//     the row is the base state's row with n columns changed; columns are
//     0 accepting, 1 lookahead, 2 + c the next state for category c.
//   u8 tries; the first: u16 n, n x (varint start delta, u8 category);
//     each other: u8 m, m x u8 (the category of the first's category k),
//     u16 n, n x (varint start delta, u8 category or 0xFF: the first's,
//     mapped): from each start on, up to the next.
//   u8 sets; each: u8 table, u8 trie.
#include <unicode/ubrk.h>
#include <unicode/ucptrie.h>
#include <unicode/uversion.h>
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <vector>

typedef std::vector<uint8_t> Bytes;

struct Table {
  uint32_t cats, states, dict, look, flags;
  std::vector<std::vector<uint16_t>> rows;  // accepting, lookahead, next...
  bool operator==(const Table& o) const {
    return cats == o.cats && dict == o.dict && look == o.look && flags == o.flags && rows == o.rows;
  }
};

struct Range { uint32_t start; uint32_t cat; };

static void die(const char* s) { fprintf(stderr, "icu-linebreak: %s\n", s); exit(1); }

static void u16(Bytes& o, uint32_t v) { o.push_back(v & 0xff); o.push_back(v >> 8); }
static void varint(Bytes& o, uint32_t v) {
  while (v >= 0x80) { o.push_back((v & 0x7f) | 0x80); v >>= 7; }
  o.push_back(v);
}

static void load(const char* loc, Table& t, std::vector<Range>& r) {
  UErrorCode st = U_ZERO_ERROR;
  UBreakIterator* bi = ubrk_open(UBRK_LINE, loc, NULL, 0, &st);
  if (U_FAILURE(st)) die("ubrk_open");
  int32_t n = ubrk_getBinaryRules(bi, NULL, 0, &st);
  st = U_ZERO_ERROR;
  Bytes v(n);
  ubrk_getBinaryRules(bi, v.data(), n, &st);
  if (U_FAILURE(st)) die("ubrk_getBinaryRules");
  uint32_t h[20];
  memcpy(h, v.data(), sizeof h);
  if (h[0] != 0xb1a0 || v[4] != 6) die("not RBBI data format 6");
  t.cats = h[3];
  const uint8_t* ft = v.data() + h[4];
  uint32_t f[5];
  memcpy(f, ft, sizeof f);
  t.states = f[0];
  t.dict = f[2];
  t.look = f[3];
  t.flags = f[4];
  bool bits8 = (t.flags & 4) != 0;
  uint32_t rowlen = f[1];
  t.rows.clear();
  for (uint32_t s = 0; s < t.states; s++) {
    const uint8_t* row = ft + 20 + rowlen * s;
    auto at = [&](uint32_t i) -> uint16_t {
      if (bits8) return row[i];
      uint16_t x; memcpy(&x, row + 2 * i, 2); return x;
    };
    std::vector<uint16_t> out;
    out.push_back(at(0));  // fAccepting
    out.push_back(at(1));  // fLookAhead (fTagsIdx at 2: unused)
    for (uint32_t c = 0; c < t.cats; c++) out.push_back(at(3 + c));
    t.rows.push_back(out);
  }
  t.flags &= ~4u;
  UCPTrie* trie = ucptrie_openFromBinary(UCPTRIE_TYPE_FAST, UCPTRIE_VALUE_BITS_ANY,
                                         v.data() + h[8], h[9], NULL, &st);
  if (U_FAILURE(st)) die("ucptrie_openFromBinary");
  r.clear();
  UChar32 c = 0;
  while (c <= 0x10ffff) {
    uint32_t val;
    UChar32 e = ucptrie_getRange(trie, c, UCPMAP_RANGE_NORMAL, 0, NULL, NULL, &val);
    if (val >= 0xff) die("category too large");
    r.push_back({(uint32_t)c, val});
    c = e + 1;
  }
  ucptrie_close(trie);
  ubrk_close(bi);
}

static void write_table(Bytes& o, const Table& t) {
  o.push_back(t.cats);
  u16(o, t.states);
  o.push_back(t.dict);
  o.push_back(t.look);
  o.push_back(t.flags);
  for (size_t i = 0; i < t.rows.size(); i++) {
    const auto& r = t.rows[i];
    auto diff = [&](const std::vector<uint16_t>* b) {
      size_t n = 0;
      for (size_t k = 0; k < r.size(); k++) n += r[k] != (b ? (*b)[k] : 0);
      return n;
    };
    size_t best = diff(NULL), base = 0xffff;
    for (size_t j = 0; j < i; j++) {
      size_t d = diff(&t.rows[j]);
      if (d < best) { best = d; base = j; }
    }
    u16(o, base);
    o.push_back(best);
    for (size_t k = 0; k < r.size(); k++) {
      uint16_t b = base == 0xffff ? 0 : t.rows[base][k];
      if (r[k] != b) { o.push_back(k); u16(o, r[k]); }
    }
  }
}

static uint32_t cat_at(const std::vector<Range>& r, uint32_t cp) {
  size_t lo = 0, hi = r.size();
  while (hi - lo > 1) {
    size_t mid = (lo + hi) / 2;
    if (r[mid].start <= cp) lo = mid; else hi = mid;
  }
  return r[lo].cat;
}

static void write_trie(Bytes& o, const std::vector<Range>& base, const std::vector<Range>& r, bool first) {
  if (first) {
    u16(o, r.size());
    uint32_t prev = 0;
    for (auto& x : r) { varint(o, x.start - prev); prev = x.start; o.push_back(x.cat); }
    return;
  }
  // the points where either changes, and how much of the code space each
  // pair of categories covers: each base category maps to its commonest
  std::vector<uint32_t> pts;
  for (auto& x : base) pts.push_back(x.start);
  for (auto& x : r) pts.push_back(x.start);
  std::sort(pts.begin(), pts.end());
  pts.erase(std::unique(pts.begin(), pts.end()), pts.end());
  std::map<std::pair<uint32_t, uint32_t>, uint64_t> cover;
  uint32_t m = 0;
  for (auto& x : base) m = std::max(m, x.cat + 1);
  for (size_t i = 0; i < pts.size(); i++) {
    uint32_t end = i + 1 < pts.size() ? pts[i + 1] : 0x110000;
    cover[{cat_at(base, pts[i]), cat_at(r, pts[i])}] += end - pts[i];
  }
  std::vector<uint32_t> map(m, 0);
  std::vector<uint64_t> most(m, 0);
  for (auto& kv : cover)
    if (kv.second > most[kv.first.first]) { most[kv.first.first] = kv.second; map[kv.first.first] = kv.first.second; }
  o.push_back(m);
  for (uint32_t k = 0; k < m; k++) o.push_back(map[k]);
  std::vector<Range> d;
  for (uint32_t p : pts) {
    uint32_t b = cat_at(base, p), c = cat_at(r, p);
    uint32_t v = map[b] == c ? 0xff : c;
    if (d.empty() || d.back().cat != v) d.push_back({p, v});
  }
  u16(o, d.size());
  uint32_t prev = 0;
  for (auto& x : d) { varint(o, x.start - prev); prev = x.start; o.push_back(x.cat); }
}

int main(int argc, char** argv) {
  if (argc != 2) die("usage: icu-linebreak OUT.bin");
  const char* locs[] = {"en", "en@lb=loose", "en@lb=normal", "zh", "zh@lb=loose", "zh@lb=normal"};
  std::vector<Table> tables;
  std::vector<std::vector<Range>> tries;
  Bytes sets;
  for (const char* l : locs) {
    Table t;
    std::vector<Range> r;
    load(l, t, r);
    size_t ti = 0;
    while (ti < tables.size() && !(tables[ti] == t)) ti++;
    if (ti == tables.size()) tables.push_back(t);
    size_t ri = 0;
    while (ri < tries.size() && !(tries[ri].size() == r.size() &&
                                  std::equal(r.begin(), r.end(), tries[ri].begin(),
                                             [](const Range& a, const Range& b) { return a.start == b.start && a.cat == b.cat; })))
      ri++;
    if (ri == tries.size()) tries.push_back(r);
    sets.push_back(ti);
    sets.push_back(ri);
  }
  UVersionInfo ver;
  u_getVersion(ver);
  Bytes o = {'L', 'B', ver[0], 0};
  o.push_back(tables.size());
  for (auto& t : tables) write_table(o, t);
  o.push_back(tries.size());
  for (size_t i = 0; i < tries.size(); i++) write_trie(o, tries[0], tries[i], i == 0);
  o.push_back(sets.size() / 2);
  o.insert(o.end(), sets.begin(), sets.end());
  FILE* f = fopen(argv[1], "wb");
  if (!f) die("cannot write");
  fwrite(o.data(), 1, o.size(), f);
  fclose(f);
  fprintf(stderr, "%zu tables, %zu tries, %zu bytes\n", tables.size(), tries.size(), o.size());
  return 0;
}
