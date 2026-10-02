//! Language-independent lexical features shared by every AX ranking path.
//!
//! Skill routing and memory retrieval both need to compare a short query with
//! stored text. They share this module so the two never drift apart: text is
//! NFKC-normalized, case-folded and reduced to word-like tokens plus Unicode
//! character n-grams, and compared with a similarity normalized to
//! `0.0..=1.0`. Nothing here detects or special-cases a language — Chinese,
//! Japanese, English and any mix of them go through the same code path.
//!
//! The score is a **candidate ranking signal**, not a semantic judgement:
//! callers pair it with their own confidence thresholds and keep low-confidence
//! decisions with the model instead of turning a lexical score into a verdict.

use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

/// Longest normalized prefix used to build features.
///
/// Ranking only needs the intent at the start of a text, so a pasted document
/// cannot make a per-turn comparison expensive.
pub const MAX_FEATURE_CHARS: usize = 2_000;

/// Feature families compared by [`LexicalFeatures::similarity`], in this
/// order: word tokens, then character n-grams of the sizes below.
///
/// Word tokens carry the least weight: they are the smallest sets, so a single
/// common word ("code") would otherwise read as a strong match, while n-grams
/// describe how much of the phrase is actually shared.
const FAMILY_WEIGHTS: [f64; 4] = [0.20, 0.35, 0.25, 0.20];
/// Character n-gram sizes built from every token, aligned with the last
/// three [`FAMILY_WEIGHTS`].
///
/// Bigrams carry most of the signal for scripts without spaces (中文/日本語),
/// where a two-character word is the common case; trigrams and fourgrams add
/// precision for longer shared sequences in every script.
const NGRAM_SIZES: [usize; 3] = [2, 3, 4];
/// Tokens longer than this are represented by their n-grams only, so a long
/// unsegmented run or a pasted identifier cannot dominate the word set.
const MAX_WORD_CHARS: usize = 32;

/// Set-based features of one text, computed once per stored item or per query.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LexicalFeatures {
    words: BTreeSet<String>,
    bigrams: BTreeSet<String>,
    trigrams: BTreeSet<String>,
    fourgrams: BTreeSet<String>,
}

impl LexicalFeatures {
    /// Normalizes `text` and extracts its word and n-gram sets.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut features = Self::default();
        for token in normalize(text).split(|c: char| !c.is_alphanumeric()) {
            if token.is_empty() {
                continue;
            }
            features.push_word(token);
            for (index, size) in NGRAM_SIZES.iter().enumerate() {
                features.push_grams(token, *size, index + 1);
            }
        }
        features
    }

    /// Similarity in `0.0..=1.0`: weighted overlap coefficient over the four
    /// families. Identical feature sets score `1.0`; disjoint sets score `0.0`.
    #[must_use]
    pub fn similarity(&self, other: &Self) -> f64 {
        let left = self.families();
        let right = other.families();
        let mut score = 0.0;
        for (index, weight) in FAMILY_WEIGHTS.iter().enumerate() {
            score += weight * overlap(left[index], right[index]);
        }
        score
    }

    /// Whether the text produced no features at all (empty or punctuation only).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.bigrams.is_empty()
    }

    fn families(&self) -> [&BTreeSet<String>; 4] {
        [&self.words, &self.bigrams, &self.trigrams, &self.fourgrams]
    }

    fn push_word(&mut self, token: &str) {
        if token.chars().count() <= MAX_WORD_CHARS {
            self.words.insert(token.to_owned());
        }
    }

    fn push_grams(&mut self, token: &str, size: usize, family: usize) {
        let chars = token.chars().collect::<Vec<_>>();
        if chars.len() < size {
            return;
        }
        for window in chars.windows(size) {
            let gram = window.iter().collect::<String>();
            if let Some(set) = self.family_mut(family) {
                set.insert(gram);
            }
        }
    }

    fn family_mut(&mut self, family: usize) -> Option<&mut BTreeSet<String>> {
        match family {
            1 => Some(&mut self.bigrams),
            2 => Some(&mut self.trigrams),
            3 => Some(&mut self.fourgrams),
            _ => None,
        }
    }
}

/// NFKC, then case fold. The character limit is applied before lowercasing,
/// which may expand a character but never grows unbounded.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.nfkc()
        .take(MAX_FEATURE_CHARS)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Overlap coefficient (Szymkiewicz–Simpson): the share of the *smaller* set
/// that both sets have in common.
///
/// Callers routinely compare a short query against longer stored text, so a
/// symmetric measure is the wrong tool: Sørensen–Dice divides by the sum of
/// both sizes, which caps a short Chinese query at ~0.3 even when its whole
/// wording appears in the stored text, and that cap cannot be separated from
/// the noise of a single shared English word. Normalizing by the smaller side
/// makes "the query is contained in the stored text" score `1.0` and keeps the
/// score comparable across scripts and text lengths.
fn overlap(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left.intersection(right).count();
    if shared == 0 {
        return 0.0;
    }
    ratio(shared, left.len().min(right.len()))
}

/// Feature sets stay far below 2^53 elements, so the cast is exact.
#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: usize, denominator: usize) -> f64 {
    numerator as f64 / denominator as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn similarity(left: &str, right: &str) -> f64 {
        LexicalFeatures::from_text(left).similarity(&LexicalFeatures::from_text(right))
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn identical_text_scores_one_and_unrelated_scores_zero() {
        assert_close(similarity("code review", "code review"), 1.0);
        assert_close(similarity("帮我评审代码", "帮我评审代码"), 1.0);
        assert_close(similarity("deploy the app", "量子计算综述"), 0.0);
        assert_close(similarity("", "code review"), 0.0);
    }

    #[test]
    fn normalization_is_unicode_and_language_agnostic() {
        assert_close(similarity("ＡＸ　ＲＥＶＩＥＷ", "ax review"), 1.0);
        assert_close(similarity("カタカナ", "ｶﾀｶﾅ"), 1.0);
        assert_close(similarity("Code-Review", "code review"), 1.0);
    }

    #[test]
    fn shares_signal_across_scripts() {
        assert!(similarity("帮我评审这段代码", "代码评审：检查变更与提交，发现回归。") > 0.05);
        assert!(similarity("ドキュメントの影を除去", "影の除去と文書画像処理") > 0.0);
        assert!(similarity("review 这份 spec 的 diff", "审查 code diff 与提交流程") > 0.05);
        assert!(similarity("帮我评审这段代码", "部署 k8s 集群到生产环境") < 0.05);
    }

    #[test]
    fn a_contained_query_scores_one_but_a_spread_query_does_not() {
        assert_close(
            similarity("帮我评审代码", "帮我评审代码，检查变更，发现回归"),
            1.0,
        );
        assert!(similarity("帮我评审代码", "帮我看看这个提交有没有问题") < 0.3);
    }

    #[test]
    fn single_characters_and_punctuation_carry_no_signal() {
        assert!(LexicalFeatures::from_text("  ...  ").is_empty());
        assert!(LexicalFeatures::from_text("!").is_empty());
        assert!(!LexicalFeatures::from_text("c").is_empty());
    }

    #[test]
    fn long_input_is_bounded() {
        let long = format!("code review {}", "x".repeat(MAX_FEATURE_CHARS * 2));
        let features = LexicalFeatures::from_text(&long);
        assert!(features.words.contains("code"));
        assert!(!features.words.contains(&"x".repeat(MAX_WORD_CHARS + 1)));
        assert!(features.bigrams.len() <= MAX_FEATURE_CHARS);
    }
}
