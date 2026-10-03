/* What pdfTeX's writepng.c does with libpng (read_png_info, then
 * write_png's transformations and rows), printed for comparison with
 * partex's port (crates/partex-engine/examples/png_dump.rs).
 *
 *   harness [-x] FILE...
 *
 * For each file: the info png_read_info leaves, then for each set of
 * transformations pdfTeX can ask for (tRNS to alpha iff tRNS is valid;
 * strip alpha or not with an alpha channel; strip 16 or not at 16 bits)
 * the transformed format and an FNV-1a hash of the rows (-x: the rows in
 * hex), or libpng's error.
 */
#include <png.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char errbuf[1024];
static int hex;

static void error_fn(png_structp p, png_const_charp msg)
{
    snprintf(errbuf, sizeof errbuf, "%s", msg);
    longjmp(png_jmpbuf(p), 1);
}

static void warn_fn(png_structp p, png_const_charp msg)
{
    (void)p;
    (void)msg;
}

static uint64_t fnv(uint64_t h, const unsigned char *p, size_t n)
{
    while (n--) {
        h ^= *p++;
        h *= 0x100000001b3ULL;
    }
    return h;
}

/* Buffers outside the setjmp frame. */
static unsigned char *row;
static unsigned char **rows;
static png_uint_32 nrows;

static void free_rows(void)
{
    free(row);
    row = NULL;
    if (rows != NULL) {
        for (png_uint_32 i = 0; i < nrows; i++)
            free(rows[i]);
        free(rows);
        rows = NULL;
    }
    nrows = 0;
}

/* png_create_read_struct ... png_read_info as read_png_info does it. */
static png_structp start(const char *path, png_infop *info, FILE **fp)
{
    *fp = fopen(path, "rb");
    if (*fp == NULL) {
        printf("cannot open\n");
        return NULL;
    }
    png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, NULL, error_fn, warn_fn);
    *info = png_create_info_struct(png);
    return png;
}

/* The info: 1 if it was read (and the transformation sets to try). */
static int show_info(const char *path, int *trns, int *alpha, int *b16)
{
    FILE *fp;
    png_infop info;
    png_structp png = start(path, &info, &fp);
    int ok = 0;
    if (png == NULL)
        return 0;
    if (setjmp(png_jmpbuf(png))) {
        printf("info error: %s\n", errbuf);
        goto done;
    }
    png_set_option(png, PNG_MAXIMUM_INFLATE_WINDOW, PNG_OPTION_ON);
    png_init_io(png, fp);
    png_read_info(png, info);
    {
        png_uint_32 valid = png_get_valid(png, info, 0xffffffffU);
        int color = png_get_color_type(png, info);
        int depth = png_get_bit_depth(png, info);
        png_fixed_point gamma = 0;
        png_uint_32 px, py;
        int unit;
        printf("info: %ux%u depth %d color %d interlace %d valid %x trns-valid %d",
               png_get_image_width(png, info), png_get_image_height(png, info), depth, color,
               png_get_interlace_type(png, info), valid,
               png_get_valid(png, info, PNG_INFO_tRNS) != 0);
        if (png_get_gAMA_fixed(png, info, &gamma))
            printf(" gamma %d", gamma);
        if (png_get_pHYs(png, info, &px, &py, &unit))
            printf(" phys %u,%u,%d", px, py, unit);
        printf("\n");
        png_colorp palette;
        int num_palette;
        if (png_get_PLTE(png, info, &palette, &num_palette)) {
            printf("palette: %d ", num_palette);
            for (int i = 0; i < num_palette; i++)
                printf("%02x%02x%02x", palette[i].red, palette[i].green, palette[i].blue);
            printf("\n");
        }
        png_bytep trans_alpha;
        int num_trans;
        png_color_16p trans_color;
        if (valid & PNG_INFO_tRNS) {
            png_get_tRNS(png, info, &trans_alpha, &num_trans, &trans_color);
            printf("trns: %d ", num_trans);
            if (color == PNG_COLOR_TYPE_PALETTE)
                for (int i = 0; i < num_trans; i++)
                    printf("%02x", trans_alpha[i]);
            else
                printf("%u,%u,%u,%u", trans_color->gray, trans_color->red, trans_color->green,
                       trans_color->blue);
            printf("\n");
        }
        *trns = png_get_valid(png, info, PNG_INFO_tRNS) != 0;
        *alpha = (color & PNG_COLOR_MASK_ALPHA) != 0;
        *b16 = depth == 16;
        ok = 1;
    }
done:
    png_destroy_read_struct(&png, &info, NULL);
    fclose(fp);
    return ok;
}

/* write_png's reading, with these transformations. */
static void show_image(const char *path, int t, int a, int s)
{
    FILE *fp;
    png_infop info;
    png_structp png = start(path, &info, &fp);
    if (png == NULL)
        return;
    if (setjmp(png_jmpbuf(png))) {
        printf("image %d%d%d: error: %s\n", t, a, s, errbuf);
        goto done;
    }
    png_set_option(png, PNG_MAXIMUM_INFLATE_WINDOW, PNG_OPTION_ON);
    png_init_io(png, fp);
    png_read_info(png, info);
    if (t)
        png_set_tRNS_to_alpha(png);
    if (a)
        png_set_strip_alpha(png);
    if (s)
        png_set_strip_16(png);
    (void)png_set_interlace_handling(png);
    png_read_update_info(png, info);
    {
        png_uint_32 h = png_get_image_height(png, info);
        size_t rb = png_get_rowbytes(png, info);
        int interlace = png_get_interlace_type(png, info);
        uint64_t hash = 0xcbf29ce484222325ULL;
        if (interlace == PNG_INTERLACE_NONE) {
            row = calloc(rb, 1);
            for (png_uint_32 i = 0; i < h; i++) {
                png_read_row(png, row, NULL);
                hash = fnv(hash, row, rb);
                if (hex) {
                    for (size_t k = 0; k < rb; k++)
                        printf("%02x", row[k]);
                    printf("\n");
                }
            }
        } else {
            rows = calloc(h, sizeof *rows);
            nrows = h;
            for (png_uint_32 i = 0; i < h; i++)
                rows[i] = calloc(rb, 1);
            png_read_image(png, rows);
            for (png_uint_32 i = 0; i < h; i++) {
                hash = fnv(hash, rows[i], rb);
                if (hex) {
                    for (size_t k = 0; k < rb; k++)
                        printf("%02x", rows[i][k]);
                    printf("\n");
                }
            }
        }
        printf("image %d%d%d: depth %d color %d rowbytes %zu interlace %d fnv %016llx\n", t, a, s,
               png_get_bit_depth(png, info), png_get_color_type(png, info), rb, interlace,
               (unsigned long long)hash);
    }
done:
    free_rows();
    png_destroy_read_struct(&png, &info, NULL);
    fclose(fp);
}

int main(int argc, char **argv)
{
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-x") == 0) {
            hex = 1;
            continue;
        }
        int trns, alpha, b16;
        printf("== %s\n", argv[i]);
        if (!show_info(argv[i], &trns, &alpha, &b16))
            continue;
        for (int a = 0; a <= alpha; a++)
            for (int s = 0; s <= b16; s++)
                show_image(argv[i], trns, a, s);
    }
    return 0;
}
