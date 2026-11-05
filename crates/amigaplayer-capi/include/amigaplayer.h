/*
 * amigaplayer.h - C API of the amigaplayer-rs open replayers.
 *
 * Plays Amiga music modules (ProTracker and about a hundred other formats, on an emulated
 * Paula), the PC tracker formats of libxmp and AIFF recordings, from memory to PCM. No audio
 * device, no files: the caller hands over bytes and receives samples.
 *
 * Link with libamigaplayer.so / libamigaplayer.a (cargo build --release -p amigaplayer-capi).
 * A static link needs -lpthread -ldl -lm on Linux.
 *
 * Copyright (C) the amigaplayer-rs authors. GPL-2.0-or-later: linking this library makes
 * your program a combined work that must be distributed under GPL-2.0-or-later compatible
 * terms (see LICENSE and the LICENSE-* files of the repository).
 *
 * Conventions
 *  - An ap_song is not thread-safe; different songs may be used from different threads.
 *  - Functions that return a pointer return NULL on failure, functions that return int
 *    return a negative number. The reason is available from ap_last_error() and
 *    ap_last_error_code() on the same thread, until the next failing call.
 *  - Output is interleaved stereo: a "frame" is one left and one right sample.
 *  - Strings are UTF-8. Rendering is deterministic: same input, same samples.
 *  - API stability: 0.x. ap_version() gives major*10000 + minor*100 + patch.
 */
#ifndef AMIGAPLAYER_H
#define AMIGAPLAYER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle of an open module. */
typedef struct ApSong ap_song;

/* Values of ap_last_error_code(); 0 means no error. More may be added. */
enum {
    AP_ERR_NOT_RECOGNISED = 1, /* no replayer recognises the data */
    AP_ERR_INVALID = 2,        /* recognised but cannot be played (corrupt, unsupported variant) */
    AP_ERR_RATE = 3,           /* sample rate outside 1000..768000 Hz */
    AP_ERR_SUBSONG = 4,        /* subsong out of range */
    AP_ERR_INTERNAL = 5,       /* a replayer failed internally; the song is unusable */
    AP_ERR_ARGUMENT = 6        /* NULL or malformed argument */
};

/* The version of this API: major * 10000 + minor * 100 + patch (0.1.0 is 100). */
int ap_version(void);

/* The message of the last failed call on this thread (empty if none). */
const char *ap_last_error(void);
/* The AP_ERR_* code of the last failed call on this thread, 0 if none. */
int ap_last_error_code(void);

/*
 * Which format would play data[0..len)? Copies the NUL-terminated name into out (cut to cap
 * bytes including the NUL; out may be NULL with cap 0). Returns the full length of the name
 * without the NUL, or 0 if no replayer recognises the data.
 */
size_t ap_detect(const uint8_t *data, size_t len, char *out, size_t cap);

/*
 * Opens a module held in memory. name is its file name (a hint some formats need; a path is
 * cut to its last component). rate is the output rate in Hz, 1000..768000 (44100 or 48000).
 * Returns NULL on failure. The data is copied: it need not outlive the call.
 */
ap_song *ap_open(const char *name, const uint8_t *data, size_t len, uint32_t rate);

/*
 * As ap_open, with n companion files in memory (TFMX smpl.*, IFF-SMUS Instruments/ files):
 * paths[i] is the path relative to the module (e.g. "smpl.tune"), datas[i] / lens[i] its
 * content.
 */
ap_song *ap_open_with(const char *name, const uint8_t *data, size_t len, uint32_t rate,
                      size_t n, const char *const *paths, const uint8_t *const *datas,
                      const size_t *lens);

/* Closes a song and frees its memory. NULL is allowed. */
void ap_close(ap_song *song);

/*
 * Renders up to `frames` frames (2 * frames samples are written) and returns the number of
 * frames written (frames, unless an argument is NULL, then 0). ap_render_f32 gives samples
 * in -1.0..1.0.
 */
size_t ap_render_s16(ap_song *song, int16_t *out, size_t frames);
size_t ap_render_f32(ap_song *song, float *out, size_t frames);

/* The first, last and current subsong (any pointer may be NULL). Returns 0, -1 for NULL song. */
int ap_subsongs(ap_song *song, uint16_t *first, uint16_t *last, uint16_t *current);
/* Restarts at subsong n. Returns 0, or -1 (AP_ERR_SUBSONG) if n is outside first..last. */
int ap_set_subsong(ap_song *song, uint16_t n);

/*
 * 1 if the replayer has reported the end of the song, else 0. Many Amiga players never do:
 * they loop for ever, so also stop at a time limit or after a stretch of silence.
 */
int ap_songend(ap_song *song);
/* Seconds rendered since the subsong started. */
double ap_elapsed_secs(ap_song *song);
/* The length in seconds when the format states it (recordings, PC trackers), else < 0. */
double ap_duration_hint(ap_song *song);
/* The sample rate the song was opened at. */
uint32_t ap_sample_rate(ap_song *song);

/* Starts the current subsong again from its beginning. Returns 0 or -1. */
int ap_restart(ap_song *song);
/* Positions the song secs seconds into the current subsong (by rendering up to there). */
int ap_seek(ap_song *song, double secs);

/*
 * Metadata. The returned strings belong to the song: valid until the next call of the same
 * function on that song or ap_close. title may be empty; it falls back to the file name.
 */
const char *ap_title(ap_song *song);
const char *ap_format(ap_song *song);

#ifdef __cplusplus
}
#endif

#endif /* AMIGAPLAYER_H */
