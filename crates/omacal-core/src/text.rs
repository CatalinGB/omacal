//! A single case- and accent-insensitive fold, shared by the two search paths.
//!
//! **One definition, two callers.** Events are matched in SQL
//! (`omacal_store::search_events`), tasks in Rust (`matches_query`); a fold
//! written twice would drift, so both call [`fold`] — the SQL side through a
//! scalar function registered on the connection.

use unicode_normalization::UnicodeNormalization;

/// Whether `c` is a combining *diacritical* mark — the accents this fold
/// exists to drop.
///
/// **The cut is the combining diacritical blocks, not the whole of Unicode
/// general category `M` (Mn, Mc, Me).** In a script where a mark is part of a
/// letter, category `M` also covers spelling, not accents: a Devanagari matra,
/// a Thai vowel sign or an Arabic harakah. Dropping those would fold `दिल`
/// (*dil*) and `दाल` (*dal*) together, and over-match. This keeps them.
fn is_diacritic(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}'
    )
}

/// Fold `s` to a form two strings can be compared for equality or containment:
/// lowercased, with combining diacritics decomposed away.
///
/// `nfc` first, so text that arrived decomposed has its marks attached before
/// the case fold; then lowercasing (Unicode, not ASCII); then `nfd`, dropping
/// every combining diacritical mark. `ușă` and `usa` fold alike; so do `Élise`
/// and `elise`.
///
/// **Only the combining diacritical blocks are dropped** (U+0300–036F and its
/// Extended/Supplement), not every mark: a spelling mark such as an Indic matra
/// or an Arabic harakah is left alone (see [`is_diacritic`]). Characters that do
/// not decompose at all — Turkish `ı`, `ß`, `ł`, `ø` — keep their identity:
/// this is not `deunicode`, and it does not transliterate.
pub fn fold(s: &str) -> String {
    s.nfc()
        .flat_map(char::to_lowercase)
        .nfd()
        .filter(|c| !is_diacritic(*c))
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

    /// Marks that are spelling, not accents, are left alone: the Devanagari
    /// vowel sign `ि` (U+093F), the Thai vowel sign `ี` (U+0E35) and the Arabic
    /// kasra `ِ` (U+0650) are all category `M`, but outside the diacritical
    /// blocks, so `दिल` (*dil*) and `दाल` (*dal*) stay distinct. Dropping all of
    /// category `M` would fold both to `दल`.
    #[test]
    fn a_spelling_mark_is_not_an_accent() {
        assert_eq!(fold("दिल"), "दिल");
        assert_ne!(fold("दिल"), fold("दाल"));
        assert_eq!(fold("ดี"), "ดี", "a Thai vowel sign keeps its identity");
        assert_eq!(fold("بِت"), "بِت", "an Arabic harakah keeps its identity");
    }

    /// Where the mark *is* an accent it still folds — the consequence the
    /// review accepted. `й` is `и` + U+0306 and `ё` is `е` + U+0308, both in
    /// the diacritical block, so both fold to their base letter (and case folds
    /// on top, as everywhere).
    #[test]
    fn cyrillic_short_i_and_yo_still_fold() {
        assert_eq!(fold("й"), "и");
        assert_eq!(fold("ё"), "е");
        assert_eq!(fold("Йогурт"), "иогурт");
    }
}
