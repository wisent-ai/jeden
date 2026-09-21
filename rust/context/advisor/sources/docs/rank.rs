//! Ranking a documentation corpus against a query.
//!
//! Term weights are measured, not opinions: a word occurring in nearly every
//! section separates nothing and weighs nearly nothing, which is why no list
//! of words to ignore exists anywhere in this module.

use super::corpus::Section;
use crate::context::advisor::text::stem;

pub(super) const NO_MATCH: f64 = 0.0;

/// Inverse document frequency over the corpus just read.
pub(super) fn weights(sections: &[Section], terms: &[String]) -> Vec<f64> {
    let total = sections.len() as f64;
    terms
        .iter()
        .map(|term| {
            let frequency = sections
                .iter()
                .filter(|section| section.contains(term))
                .count() as f64;
            (1.0 + total / (1.0 + frequency)).ln()
        })
        .collect()
}

/// Terms carrying at least half the weight of the strongest one — the words
/// a snippet should be cut around.
pub(super) fn informative_terms(terms: &[String], weights: &[f64]) -> Vec<String> {
    let strongest = weights.iter().copied().fold(NO_MATCH, f64::max);
    if strongest <= NO_MATCH {
        return terms.to_vec();
    }
    terms
        .iter()
        .zip(weights)
        .filter(|(_, weight)| **weight >= strongest / 2.0)
        .map(|(term, _)| term.clone())
        .collect()
}

/// Weighted term hits, with a title or path hit worth more than a body hit,
/// scaled by how much of the query the section covers and damped by its
/// length so a long file cannot win by repetition alone. A stem hit counts
/// for half: `routingu` should find `routing`, but an exact match is still
/// the better answer.
pub(super) fn score_section(section: &Section, terms: &[String], weights: &[f64]) -> f64 {
    const STEM_WEIGHT: f64 = 0.5;
    const LENGTH_SCALE: f64 = 2_000.0;
    let fields = Fields {
        body: section.body.to_lowercase(),
        title: section.title.to_lowercase(),
        path: section.path.display().to_string().to_lowercase(),
    };
    let mut total = NO_MATCH;
    let mut covered = usize::default();
    for (term, weight) in terms.iter().zip(weights) {
        let exact = fields.hits(term);
        let stemmed = stem(term);
        let approximate = if stemmed == *term {
            NO_MATCH
        } else {
            STEM_WEIGHT * fields.hits(&stemmed)
        };
        if exact + approximate > NO_MATCH {
            covered += 1;
            total += weight * (exact + approximate);
        }
    }
    if covered == usize::default() {
        return NO_MATCH;
    }
    let coverage = covered as f64 / terms.len() as f64;
    let length_norm = 1.0 + (section.body.chars().count() as f64 / LENGTH_SCALE);
    total * coverage / length_norm
}

struct Fields {
    body: String,
    title: String,
    path: String,
}

impl Fields {
    fn hits(&self, needle: &str) -> f64 {
        const BODY_CAP: usize = 8;
        const TITLE_WEIGHT: f64 = 4.0;
        const PATH_WEIGHT: f64 = 3.0;
        let body = self.body.matches(needle).count().min(BODY_CAP) as f64;
        let title = self.title.matches(needle).count() as f64;
        let path = self.path.matches(needle).count() as f64;
        body + TITLE_WEIGHT * title + PATH_WEIGHT * path
    }
}
