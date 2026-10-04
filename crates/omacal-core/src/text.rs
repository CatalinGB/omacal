//! A single case- and accent-insensitive fold, shared by the two search paths.
//!
//! **One definition, two callers.** Events are matched in SQL
//! (`omacal_store::search_events`), tasks in Rust (`matches_query`); a fold
//! written twice would drift, so both call [`fold`] — the SQL side through a
//! scalar function registered on the connection.

use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// Fold `s` to a form two strings can be compared for equality or containment:
/// lowercased, with combining marks decomposed away.
///
/// `nfc` first, so text that arrived decomposed has its marks attached before
/// the case fold; then lowercasing (Unicode, not ASCII); then `nfd`, dropping
/// every combining mark. `ușă` and `usa` fold alike; so do `Élise` and `elise`.
///
/// **The cut is `is_combining_mark` — Unicode general category `M` (Mn, Mc,
/// Me), not "accents" specifically.** In the Latin and Cyrillic text this app
/// matches, those are the diacritics, so the effect is "ignore accents"; in a
/// script where a mark is part of a letter (an Indic matra, an Arabic harakah),
/// it goes too. Characters that do not decompose at all — Turkish `ı`, `ß`,
/// `ł`, `ø` — keep their identity: this is not `deunicode`, and it does not
/// transliterate.
pub fn fold(s: &str) -> String {
    s.nfc()
        .flat_map(char::to_lowercase)
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn accents_fold_away() {
        assert_eq!(fold("ușă"), "usa");
        assert_eq!(fold("Garnituri ușă intrare"), "garnituri usa intrare");
        assert_eq!(fold("Élise"), "elise");
        assert_eq!(fold("Târg"), "targ");
        // U+0130 decomposes to `i` + a dot, which is dropped.
        assert_eq!(fold("İstanbul"), "istanbul");
    }

    #[test]
    fn case_folds_too() {
        assert_eq!(fold("BOARD"), "board");
    }

    /// The named limits — these keep their identity on purpose, and are here so
    /// nobody later mistakes them for bugs.
    #[test]
    fn the_undecomposable_keeps_its_identity() {
        assert_ne!(fold("ı"), "i");
        assert_eq!(fold("ß"), "ß");
        assert_eq!(fold("ł"), "ł");
        assert_eq!(fold("ø"), "ø");
    }
}
