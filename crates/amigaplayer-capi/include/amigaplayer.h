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
    AP_ERR_ARGUMENT = 6,       /* NULL or malformed argument */
    AP_ERR_GUEST = 7,          /* the 68k code of a custom module failed; the song is silent */
    AP_ERR_UNSUPPORTED = 8     /* the library was built without what was asked for */
};

/* Flags of ap_open_opts(). */
enum {
    /*
     * Custom modules may play: executables that carry their own replay routine (CUST.*
     * files), whose 68k code is run on the emulated 68000. That code sees the companion
     * files handed to ap_open_opts() and no file of the host, and every call into it has an
     * instruction budget: a module that hangs or faults stops, renders silence and reports
     * through ap_error(). No external player is ever loaded. Needs a library built with the
     * host-68k feature (the default), else AP_ERR_UNSUPPORTED.
     */
    AP_OPEN_CUSTOM = 1
};

/* The version of this API: major * 10000 + minor * 100 + patch (0.2.0 is 200). */
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
 * As ap_open, with n companion files in memory (TFMX smpl.NAME, IFF-SMUS Instruments/ files):
 * paths[i] is the path relative to the module (e.g. "smpl.tune"), datas[i] / lens[i] its
 * content.
 */
ap_song *ap_open_with(const char *name, const uint8_t *data, size_t len, uint32_t rate,
                      size_t n, const char *const *paths, const uint8_t *const *datas,
                      const size_t *lens);

/*
 * As ap_open_with, with flags (AP_OPEN_*, or 0 for what ap_open_with does). New in 0.2.0.
 */
ap_song *ap_open_opts(const char *name, const uint8_t *data, size_t len, uint32_t rate,
                      size_t n, const char *const *paths, const uint8_t *const *datas,
                      const size_t *lens, uint32_t flags);

/*
 * 1 if data[0..len) is a custom module (which ap_open_opts plays with AP_OPEN_CUSTOM, unless
 * ap_detect names a replayer for it: that one plays it then), else 0. Nothing of the module
 * is run. New in 0.2.0.
 */
int ap_detect_custom(const uint8_t *data, size_t len);

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

/*
 * Why the song has stopped by itself and renders silence, or NULL while all is well: the 68k
 * code of a custom module failed (AP_ERR_GUEST: a CPU exception, a call that did not return
 * within its instruction budget), or a replayer did (AP_ERR_INTERNAL). ap_songend() stays 0:
 * check this after rendering, as you check ap_songend(). The string is valid until the next
 * ap_error call on that song; ap_error_code gives the AP_ERR_* code, 0 for none.
 * ap_restart() starts the song again. New in 0.2.0.
 */
const char *ap_error(ap_song *song);
int ap_error_code(ap_song *song);

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

/*
 * The song database of audacious-uade: for some 476,000 module files, the length of each
 * subsong, the format, and author, album, publisher and year. New in 0.2.0.
 *
 * Only in a library built with the songdb feature
 *     cargo build --release -p amigaplayer-capi --features songdb
 * (without it the functions are there, and ap_songdb_open fails with AP_ERR_UNSUPPORTED).
 * The database is NOT part of the library: it is the directory conf/songdb of
 * https://github.com/mvtiaine/audacious-uade (xxh32idx.tsv, songlengths.tsv, modinfos.tsv,
 * metadata.tsv; installed by its packages as share/audacious-uade/songdb), licensed
 * CC BY-NC-SA 4.0, and is read from the directory you name. The reader's code is a port of
 * audacious-uade's (Matti Tiainen), LGPL-2.1-or-later OR GPL-2.0-or-later: crates/songdb.
 *
 * A file is found by its bytes (as they are on disk, not unpacked): the key is the XXH32 of
 * the first 256 KiB and the low 16 bits of the length, 12 hex digits. The lengths were
 * measured with other replayers: good for a progress bar, approximate as a place to cut.
 */
typedef struct ApSongDb ap_songdb;

/* How a song ended when its length was measured (ap_songdb_subsong.status). */
enum {
    AP_SONGEND_ERROR = -1,
    AP_SONGEND_NONE = 0,
    AP_SONGEND_PLAYER = 1,               /* the player reported the end */
    AP_SONGEND_TIMEOUT = 2,
    AP_SONGEND_SILENCE = 3,
    AP_SONGEND_LOOP = 4,                 /* a loop was detected */
    AP_SONGEND_VOLUME = 5,
    AP_SONGEND_REPEAT = 6,
    AP_SONGEND_PLAYER_PLUS_SILENCE = 7,
    AP_SONGEND_PLAYER_PLUS_VOLUME = 8,
    AP_SONGEND_LOOP_PLUS_SILENCE = 9,
    AP_SONGEND_LOOP_PLUS_VOLUME = 10,
    AP_SONGEND_NOSOUND = 11
};

typedef struct {
    uint8_t subsong;      /* its number, as ap_subsongs() counts */
    uint8_t is_duplicate; /* 1: the same audio as an earlier subsong */
    int8_t status;        /* AP_SONGEND_* */
    uint32_t length_ms;   /* in steps of 20 ms */
} ap_songdb_subsong;

/* Strings are never NULL: "" where the database knows nothing. year and channels: 0 then. */
typedef struct {
    const char *key;
    const char *format; /* as the database names it ("AHX v2"), not ap_format()'s name */
    const char *author;
    const char *album;
    const char *publisher;
    uint16_t year;
    uint8_t channels;
    size_t subsong_count;
    const ap_songdb_subsong *subsongs; /* subsong_count entries, ascending */
} ap_songdb_info;

/*
 * Loads the database from directory dir into memory (about 19 MB and 0.2 s for the real
 * one). NULL on failure (AP_ERR_INVALID: a file is missing or does not parse). Lookups on one
 * handle must not run at the same time: use a handle per thread, or a lock.
 */
ap_songdb *ap_songdb_open(const char *dir);
/* Frees a database. NULL is allowed. */
void ap_songdb_free(ap_songdb *db);
/* The number of files the database knows. */
size_t ap_songdb_count(ap_songdb *db);

/*
 * Looks up the file data[0..len). Returns 1 and fills *out if the database knows it, 0 if
 * not (*out is left alone), -1 for a NULL argument. The strings and the subsong array of
 * *out belong to db: they are valid until the next lookup on db, or ap_songdb_free(db).
 */
int ap_songdb_lookup(ap_songdb *db, const uint8_t *data, size_t len, ap_songdb_info *out);
/* As ap_songdb_lookup, by key: 12 hex digits. Any other string is not found (0). */
int ap_songdb_lookup_key(ap_songdb *db, const char *key, ap_songdb_info *out);
/* Writes the key of data[0..len) into out: 12 hex digits and a NUL, 13 bytes. 0 or -1. */
int ap_songdb_key(const uint8_t *data, size_t len, char *out);
/* The database's name of a status: "player", "loop+silence", ...; "unknown" otherwise. */
const char *ap_songdb_status_name(int status);

#ifdef __cplusplus
}
#endif

#endif /* AMIGAPLAYER_H */
