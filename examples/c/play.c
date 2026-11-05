/*
 * Renders a module to raw signed 16-bit little-endian stereo PCM with the C API.
 *
 *   cc -O2 -Icrates/amigaplayer-capi/include examples/c/play.c -Ltarget/release -lamigaplayer \
 *      -Wl,-rpath,target/release -o play
 *   ./play tune.mod out.raw [seconds [subsong]]
 *   ffmpeg -f s16le -ar 44100 -ac 2 -i out.raw out.wav       # to listen to it
 *
 * Stops at the song end, after `seconds` (default 60) or after 10 seconds of silence.
 */
#include <amigaplayer.h>

#include <stdio.h>
#include <stdlib.h>

#define RATE 44100
#define CHUNK 2048

static uint8_t *slurp(const char *path, size_t *len)
{
    FILE *f = fopen(path, "rb");
    if (!f) { perror(path); return NULL; }
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *buf = malloc(n > 0 ? (size_t)n : 1);
    if (!buf || fread(buf, 1, (size_t)n, f) != (size_t)n) { fprintf(stderr, "%s: read failed\n", path); fclose(f); free(buf); return NULL; }
    fclose(f);
    *len = (size_t)n;
    return buf;
}

int main(int argc, char **argv)
{
    if (argc < 3) { fprintf(stderr, "usage: %s MODULE OUT.raw [SECONDS [SUBSONG]]\n", argv[0]); return 2; }
    double seconds = argc > 3 ? atof(argv[3]) : 60.0;

    size_t len;
    uint8_t *data = slurp(argv[1], &len);
    if (!data) return 1;

    char fmt[128];
    if (ap_detect(data, len, fmt, sizeof fmt) == 0) { fprintf(stderr, "%s: not recognised\n", argv[1]); return 1; }

    const char *base = argv[1];
    for (const char *p = argv[1]; *p; p++) if (*p == '/') base = p + 1;
    ap_song *song = ap_open(base, data, len, RATE);
    free(data);
    if (!song) { fprintf(stderr, "open failed (%d): %s\n", ap_last_error_code(), ap_last_error()); return 1; }

    if (argc > 4 && ap_set_subsong(song, (uint16_t)atoi(argv[4])) != 0) {
        fprintf(stderr, "subsong: %s\n", ap_last_error());
        ap_close(song);
        return 1;
    }
    uint16_t first, last, cur;
    ap_subsongs(song, &first, &last, &cur);
    fprintf(stderr, "\"%s\" [%s] subsong %u of %u..%u\n", ap_title(song), ap_format(song), cur, first, last);

    FILE *out = fopen(argv[2], "wb");
    if (!out) { perror(argv[2]); ap_close(song); return 1; }
    static int16_t buf[CHUNK * 2];
    size_t total = 0, quiet = 0;
    while (!ap_songend(song) && ap_elapsed_secs(song) < seconds) {
        size_t n = ap_render_s16(song, buf, CHUNK);
        if (n == 0) break;
        int silent = 1;
        for (size_t i = 0; i < n * 2; i++) if (buf[i]) { silent = 0; break; }
        quiet = silent ? quiet + n : 0;
        fwrite(buf, 4, n, out);
        total += n;
        if (quiet >= 10 * RATE) break;
    }
    fclose(out);
    fprintf(stderr, "%.1f s written to %s\n", (double)total / RATE, argv[2]);
    ap_close(song);
    return 0;
}
