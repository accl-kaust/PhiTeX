/* The oracle side of phitex-graphite's unit check: graphite2 (the system
 * library XeTeX runs with) over a task list, each segment's slots dumped
 * with their floats' bits (examples/gr-dump.rs prints the same for the
 * port).
 *
 *   gr2dump TASKS > OUT
 *
 * A task line: FONT<TAB>LANG<TAB>FEATS<TAB>DIR<TAB>TEXT (UTF-8); FEATS is
 * `id=value,...` (decimal ids) or `-`, LANG a tag or `-`, DIR the
 * gr_make_seg direction flags. Faces are made as HarfBuzz makes them
 * (gr_face_preloadAll, tables from the file), segments with no font. */
#include <graphite2/Font.h>
#include <graphite2/Segment.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned fb(float f) { unsigned u; memcpy(&u, &f, 4); return u; }

struct file { unsigned char *d; size_t n; };

static unsigned be32(const unsigned char *p) { return (unsigned)p[0] << 24 | p[1] << 16 | p[2] << 8 | p[3]; }

static const void *get_table(const void *h, unsigned tag, size_t *len) {
  const struct file *f = h;
  if (f->n < 12) return NULL;
  unsigned n = f->d[4] << 8 | f->d[5];
  for (unsigned i = 0; i < n && 12 + 16 * i + 16 <= f->n; i++) {
    const unsigned char *e = f->d + 12 + 16 * i;
    if (be32(e) == tag) {
      unsigned off = be32(e + 8), l = be32(e + 12);
      if ((size_t)off + l > f->n) return NULL;
      *len = l;
      return f->d + off;
    }
  }
  return NULL;
}

static char *cur_font; static struct file cur_file; static gr_face *cur_face;

static gr_face *face_for(const char *path) {
  if (cur_font && !strcmp(cur_font, path)) return cur_face;
  if (cur_face) gr_face_destroy(cur_face);
  free(cur_font); free(cur_file.d);
  cur_font = strdup(path);
  FILE *fp = fopen(path, "rb");
  if (!fp) { cur_face = NULL; return NULL; }
  fseek(fp, 0, SEEK_END); cur_file.n = ftell(fp); fseek(fp, 0, SEEK_SET);
  cur_file.d = malloc(cur_file.n);
  if (fread(cur_file.d, 1, cur_file.n, fp) != cur_file.n) cur_file.n = 0;
  fclose(fp);
  gr_face_ops ops = {sizeof(gr_face_ops), get_table, NULL};
  cur_face = gr_make_face_with_ops(&cur_file, &ops, gr_face_preloadAll);
  return cur_face;
}

static size_t utf8_to_32(const char *s, uint32_t *out) {
  size_t n = 0;
  const unsigned char *p = (const unsigned char *)s;
  while (*p) {
    uint32_t c = *p++;
    int k = 0;
    if (c >= 0xF0) { c &= 7; k = 3; } else if (c >= 0xE0) { c &= 15; k = 2; } else if (c >= 0xC0) { c &= 31; k = 1; }
    while (k-- && *p) c = c << 6 | (*p++ & 63);
    out[n++] = c;
  }
  return n;
}

int main(int argc, char **argv) {
  if (argc < 2) return 2;
  FILE *in = fopen(argv[1], "r");
  if (!in) return 1;
  static char line[1 << 16];
  static uint32_t chars[1 << 16];
  unsigned long task = 0;
  while (fgets(line, sizeof line, in)) {
    line[strcspn(line, "\n")] = 0;
    char *f[5]; char *p = line;
    for (int i = 0; i < 5; i++) { f[i] = p; p = strchr(p, '\t'); if (!p && i < 4) { f[i + 1] = NULL; break; } if (p) *p++ = 0; }
    if (!f[4]) continue;
    task++;
    gr_face *face = face_for(f[0]);
    if (!face) { printf("T%lu noface\n", task); continue; }
    unsigned lang = 0;
    if (strcmp(f[1], "-")) { char t[5] = {0}; strncpy(t, f[1], 4); lang = gr_str_to_tag(t); }
    gr_feature_val *feats = gr_face_featureval_for_lang(face, lang);
    if (strcmp(f[2], "-")) {
      char *q = f[2];
      while (*q) {
        unsigned long id = strtoul(q, &q, 10); long v = 0;
        if (*q == '=') v = strtol(q + 1, &q, 10);
        const gr_feature_ref *r = gr_face_find_fref(face, (unsigned)id);
        if (r) gr_fref_set_feature_value(r, (gr_uint16)v, feats);
        if (*q == ',') q++; else break;
      }
    }
    size_t n = utf8_to_32(f[4], chars);
    gr_segment *seg = gr_make_seg(NULL, face, 0, feats, gr_utf32, chars, n, atoi(f[3]));
    if (!seg) { printf("T%lu noseg\n", task); gr_featureval_destroy(feats); continue; }
    printf("T%lu %u %08x %08x\n", task, gr_seg_n_slots(seg), fb(gr_seg_advance_X(seg)), fb(gr_seg_advance_Y(seg)));
    for (const gr_slot *s = gr_seg_first_slot(seg); s; s = gr_slot_next_in_segment(s))
      printf(" %u %08x %08x %08x %08x %d %d %d\n", gr_slot_gid(s), fb(gr_slot_origin_X(s)), fb(gr_slot_origin_Y(s)),
             fb(gr_slot_advance_X(s, face, NULL)), fb(gr_slot_advance_Y(s, face, NULL)), gr_slot_before(s), gr_slot_after(s),
             gr_slot_can_insert_before(s));
    gr_seg_destroy(seg);
    gr_featureval_destroy(feats);
  }
  return 0;
}
