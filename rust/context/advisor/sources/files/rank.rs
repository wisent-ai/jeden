//! Ranking a corpus of chunks against a query.
//!
//! Term weights are measured, not opinions: a word occurring in nearly every
//! chunk separates nothing and weighs nearly nothing, which is why no list of
//! words to ignore exists anywhere in this module.
//!
//! Every chunk carries its own lowercased fields, so a query of a dozen terms
//! scans a large corpus without lowercasing it a dozen times.

use super::corpus::Section;
use crate::context::advisor::text::stem;

pub(super) const NO_MATCH: f64 = 0.0;

/// A query term with the stem that also matches it, prepared once.
pub(super) struct Term {
    word: String,
    stem: String,
    pub(super) weight: f64,
}

/// Inverse document frequency over the corpus just read.
pub(super) fn weigh(sections: &[Section], terms: &[String]) -> Vec<Term> {
    let total = sections.len() as f64;
    terms
        .iter()
        .map(|word| {
            let stemmed = stem(word);
            let frequency = sections
                .iter()
                .filter(|section| section.contains(word, &stemmed))
                .count() as f64;
            Term {
                word: word.clone(),
                stem: stemmed,
                weight: (1.0 + total / (1.0 + frequency)).ln(),
            }
        })
        .collect()
}

/// Terms carrying at least half the weight of the strongest one — the words
/// a snippet should be cut around.
pub(super) fn informative_terms(terms: &[Term]) -> Vec<String> {
    let strongest = terms
        .iter()
        .map(|term| term.weight)
        .fold(NO_MATCH, f64::max);
    if strongest <= NO_MATCH {
        return terms.iter().map(|term| term.word.clone()).collect();
    }
    terms
        .iter()
        .filter(|term| term.weight >= strongest / 2.0)
        .map(|term| term.word.clone())
        .collect()
}

/// Weighted term hits, with a title or path hit worth more than a body hit,
/// scaled by how much of the query's *weight* the chunk covers and damped by
/// its length so a long file cannot win by repetition alone. A stem hit
/// counts for half: `routingu` should find `routing`, but an exact match is
/// still the better answer.
///
/// Coverage is measured in weight rather than in term count for a reason a
/// query exposes immediately: "jak dziala interleave rekomendacji" has one
/// word that identifies a function and three that appear everywhere, and
/// counting terms would rank three common words above the one rare one.
pub(super) fn score_section(section: &Section, terms: &[Term]) -> f64 {
    const STEM_WEIGHT: f64 = 0.5;
    const LENGTH_SCALE: f64 = 2_000.0;
    let mut total = NO_MATCH;
    let mut covered_weight = NO_MATCH;
    let mut query_weight = NO_MATCH;
    for term in terms {
        query_weight += term.weight;
        let exact = hits(section, &term.word);
        let approximate = if term.stem == term.word {
            NO_MATCH
        } else {
            STEM_WEIGHT * hits(section, &term.stem)
        };
        if exact + approximate > NO_MATCH {
            covered_weight += term.weight;
            total += term.weight * (exact + approximate);
        }
    }
    if covered_weight <= NO_MATCH || query_weight <= NO_MATCH {
        return NO_MATCH;
    }
    let coverage = covered_weight / query_weight;
    let length_norm = 1.0 + (section.body.len() as f64 / LENGTH_SCALE);
    total * coverage / length_norm
}

fn hits(section: &Section, needle: &str) -> f64 {
    const BODY_CAP: usize = 8;
    const TITLE_WEIGHT: f64 = 4.0;
    const PATH_WEIGHT: f64 = 3.0;
    let body = section.lower_body.matches(needle).count().min(BODY_CAP) as f64;
    let title = section.lower_title.matches(needle).count() as f64;
    let path = section.lower_path.matches(needle).count() as f64;
    body + TITLE_WEIGHT * title + PATH_WEIGHT * path
}
