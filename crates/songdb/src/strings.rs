// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-common/src/strings.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/common/strings.h of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: only what the song database uses is kept (`split_view_n`, `split_view_x_n`,
// `mk_string`, `from_chars` and their tests); `split`, `split_view`, `starts_with`, `ends_with`
// and the `to_hex` helpers are left out.

//! Port of `common/strings.h`.
//!
//! # The fixed-N splitters are not `splitn`
//!
//! The header has two families of splitter. The `std::vector` ones behave
//! the obvious way (they are not vendored here). The **template `<N>` ones do
//! not**, and the song database parser uses only those:
//!
//! ```text
//! split_view<2>("a,b,c", ',')  ==  ["a", "b"]      not ["a", "b,c"]
//! ```
//!
//! The C fills at most `N` slots by scanning forward, and simply stops. It
//! never gathers the remainder into the final slot the way `splitn` does.
//! Reaching for `str::splitn` here would silently glue trailing columns onto
//! the last field of every database row.
//!
//! The second difference is the `_x` suffix: those drop the **last byte** of
//! whichever field ends at the end of input. It exists to strip the trailing
//! `'\n'` from a line read out of a `.tsv`, and it is unconditional — it does
//! not check that the byte is a newline.
//!
//! Both behaviours are reproduced, and both are pinned by tests below that
//! would fail against a `splitn`-based implementation.

/// Port of `template <int N> split_view(const std::string_view&, char)`.
///
/// Fills at most `N` fields and stops; see the module docs for why this is
/// not `splitn`. Slots past the end of the input stay empty, matching the
/// C's default-constructed `std::string_view`s.
pub fn split_view_n<const N: usize>(input: &str, separator: char) -> [&str; N] {
    debug_assert!(separator.is_ascii(), "separators are ASCII in every caller");
    let mut out = [""; N];
    let mut rest = input;
    for slot in out.iter_mut() {
        match rest.find(separator) {
            Some(pos) => {
                *slot = &rest[..pos];
                rest = &rest[pos + separator.len_utf8()..];
            }
            None => {
                *slot = rest;
                break;
            }
        }
    }
    out
}

/// Port of `template <int N> split_view_x(const std::string_view&, char)`.
///
/// As [`split_view_n`], except the field that ends at the end of the input
/// loses its final byte — the trailing `'\n'` of a database line.
///
/// # Divergence from the C (deliberate)
///
/// If the input *ends* with the separator and a slot remains, the C computes
/// the final field's length as `delim - current - 1` with `delim == current`,
/// i.e. `size_t(-1)`, and hands back a `string_view` claiming `SIZE_MAX`
/// bytes. That is undefined behaviour, not a behaviour to reproduce; this
/// returns an empty field instead. Recorded in `UPSTREAM_BUGS.md` #1.
/// It needs a final line with no newline to trigger, which the shipped
/// `.tsv` files do not have.
///
/// # Panics
///
/// If dropping the last byte would land inside a multi-byte UTF-8 sequence.
/// The C would produce a malformed string there; Rust cannot represent one,
/// so this fails loudly rather than quietly truncating to a boundary. Every
/// real caller strips an ASCII `'\n'`.
pub fn split_view_x_n<const N: usize>(input: &str, separator: char) -> [&str; N] {
    debug_assert!(separator.is_ascii(), "separators are ASCII in every caller");
    let mut out = [""; N];
    let mut rest = input;
    for slot in out.iter_mut() {
        match rest.find(separator) {
            Some(pos) => {
                *slot = &rest[..pos];
                rest = &rest[pos + separator.len_utf8()..];
            }
            None => {
                *slot = drop_last_byte(rest);
                break;
            }
        }
    }
    out
}

/// Drops one trailing byte, as the C's `delim - current - 1` does.
fn drop_last_byte(s: &str) -> &str {
    if s.is_empty() {
        // The `size_t(-1)` case; see `split_view_x_n`'s divergence note.
        return "";
    }
    s.get(..s.len() - 1)
        .expect("split_view_x would split a multi-byte UTF-8 character")
}

/// Port of the `mkString` overloads.
///
/// # The trailing-delimiter quirk
///
/// Empty elements are skipped, but the decision to append the delimiter is
/// made from the element's *index*, not from whether anything follows. So a
/// trailing empty element leaves a dangling delimiter:
///
/// ```
/// # use amigaplayer_songdb::strings::mk_string;
/// assert_eq!(mk_string(&["a", "b", ""], "-"), "a-b-");
/// assert_eq!(mk_string(&["a", "", "b"], "-"), "a-b");
/// ```
///
/// This reaches the `songlengths` column of the songdb CLI output, so it is
/// reproduced rather than tidied.
pub fn mk_string<S: AsRef<str>>(v: &[S], delimiter: &str) -> String {
    let mut res = String::new();
    let n = v.len();
    for (i, part) in v.iter().enumerate() {
        let part = part.as_ref();
        if !part.is_empty() {
            res.push_str(part);
            if i + 1 < n {
                res.push_str(delimiter);
            }
        }
    }
    res
}

/// Port of `common::from_chars<T>`.
///
/// # The unvalidated single-character fast path
///
/// The C short-circuits `s.size() == 1` to `s[0] - 48`, skipping
/// `std::from_chars` entirely — so a single non-digit yields arithmetic on
/// its ASCII code rather than an error. `'+'` gives -5, `'a'` gives 49.
/// Longer inputs go through `from_chars` and are asserted to parse cleanly.
///
/// Returns `None` where the C would trip its `assert`, so callers see the
/// failure in release builds too (the C's asserts vanish under `NDEBUG`).
pub fn from_chars<T>(s: &str) -> Option<T>
where
    T: TryFrom<i64> + std::str::FromStr,
{
    if s.len() == 1 {
        let byte = s.as_bytes()[0];
        // Faithful to the C: wrapping i64 arithmetic on the raw byte, no
        // digit check. `as i64` first so '+' (43) yields -5, not 251.
        return T::try_from(byte as i64 - 48).ok();
    }
    s.parse::<T>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinguishing test for the whole module: a `splitn`-based
    /// implementation returns `["a", "b,c"]` here and passes every other
    /// test in this file.
    #[test]
    fn fixed_n_split_stops_rather_than_gathering_the_remainder() {
        assert_eq!(split_view_n::<2>("a,b,c", ','), ["a", "b"]);
        assert_eq!(split_view_n::<2>("a,b,c,d,e", ','), ["a", "b"]);
        assert_eq!(split_view_n::<3>("a,b,c,d", ','), ["a", "b", "c"]);
    }

    #[test]
    fn fixed_n_split_leaves_missing_fields_empty() {
        assert_eq!(split_view_n::<4>("a,b", ','), ["a", "b", "", ""]);
        assert_eq!(split_view_n::<2>("solo", ','), ["solo", ""]);
        assert_eq!(split_view_n::<3>("a,,c", ','), ["a", "", "c"]);
        assert_eq!(split_view_n::<2>("", ','), ["", ""]);
    }

    #[test]
    fn x_variant_drops_the_trailing_newline_of_a_database_line() {
        // The shape songdb.cc:297 actually parses.
        assert_eq!(split_view_x_n::<2>("1 2 3\t12345\n", '\t'), ["1 2 3", "12345"]);
        assert_eq!(
            split_view_x_n::<4>("author\tpublisher\talbum\t1991\n", '\t'),
            ["author", "publisher", "album", "1991"]
        );
    }

    /// The drop is unconditional, not a newline strip. Pinning this stops a
    /// future "improvement" to `trim_end` from looking harmless.
    #[test]
    fn x_variant_drops_the_last_byte_whatever_it_is() {
        assert_eq!(split_view_x_n::<2>("a\tbcd", '\t'), ["a", "bc"]);
        assert_eq!(split_view_x_n::<1>("xyz", '\t'), ["xy"]);
    }

    #[test]
    fn x_variant_stops_early_like_the_plain_one() {
        assert_eq!(split_view_x_n::<3>("a\tb\n", '\t'), ["a", "b", ""]);
    }

    /// The documented divergence: UB in the C, empty here.
    #[test]
    fn x_variant_yields_empty_where_the_c_computes_size_max() {
        assert_eq!(split_view_x_n::<2>("a\t", '\t'), ["a", ""]);
    }

    #[test]
    #[should_panic(expected = "multi-byte UTF-8")]
    fn x_variant_refuses_to_split_a_character() {
        // 'é' is two bytes; dropping one leaves no valid `&str`.
        let _ = split_view_x_n::<1>("é", '\t');
    }

    #[test]
    fn mk_string_skips_empties_but_still_emits_a_trailing_delimiter() {
        assert_eq!(mk_string(&["a", "b", "c"], "-"), "a-b-c");
        assert_eq!(mk_string(&["a", "", "b"], "-"), "a-b");
        // The quirk. A "tidied" implementation returns "a-b".
        assert_eq!(mk_string(&["a", "b", ""], "-"), "a-b-");
        assert_eq!(mk_string::<&str>(&[], "-"), "");
        assert_eq!(mk_string(&["only"], "-"), "only");
        assert_eq!(mk_string(&["", ""], "-"), "");
    }

    #[test]
    fn from_chars_parses_multi_digit_numbers() {
        assert_eq!(from_chars::<i32>("1991"), Some(1991));
        assert_eq!(from_chars::<u8>("255"), Some(255));
        assert_eq!(from_chars::<u32>("12345"), Some(12345));
        assert_eq!(from_chars::<i32>("garbage"), None);
        assert_eq!(from_chars::<u8>("256"), None);
    }

    #[test]
    fn from_chars_reproduces_the_unvalidated_single_char_path() {
        assert_eq!(from_chars::<i32>("0"), Some(0));
        assert_eq!(from_chars::<i32>("7"), Some(7));
        // Not digits. The C returns these values rather than failing.
        assert_eq!(from_chars::<i32>("+"), Some(-5));
        assert_eq!(from_chars::<i32>("a"), Some(49));
        assert_eq!(from_chars::<i32>(" "), Some(-16));
        // ...but an unsigned target cannot hold a negative, so it fails
        // where the C would have wrapped. Callers pass `uint8_t`.
        assert_eq!(from_chars::<u8>("+"), None);
    }
}
