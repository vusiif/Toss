//! Document search and navigation operations
//!
//! This module provides read-only querying operations on parsed documents,
//! including full-text search and outline generation.

use super::models::*;

pub const MAX_SEARCH_RESULTS: usize = 4096;
pub const MAX_SEARCH_TEXT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SEARCH_QUERY_BYTES: usize = 4096;

#[derive(Debug, Default)]
pub struct SearchOutcome {
    pub results: Vec<SearchResult>,
    pub limited: bool,
    pub error: Option<String>,
}

pub fn search_document(document: &Document, query: &str) -> Vec<SearchResult> {
    search_document_with_status(document, query).results
}

/// Literal Unicode simple-case-insensitive search, without normalization or
/// locale-specific/full case folding. Match offsets always address the original
/// UTF-8 text; repeated hits share one retained text allocation.
pub fn search_document_with_status(document: &Document, query: &str) -> SearchOutcome {
    use std::sync::Arc;
    let mut outcome = SearchOutcome::default();
    if query.is_empty() {
        return outcome;
    }
    if query.len() > MAX_SEARCH_QUERY_BYTES {
        outcome.error = Some(format!(
            "Search query exceeds {MAX_SEARCH_QUERY_BYTES} bytes"
        ));
        return outcome;
    }
    let matcher = match regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .size_limit(1024 * 1024)
        .build()
    {
        Ok(matcher) => matcher,
        Err(err) => {
            outcome.error = Some(format!("Cannot prepare search: {err}"));
            return outcome;
        }
    };
    let mut retained = 0usize;
    for (element_index, element) in document.elements.iter().enumerate() {
        let mut part_index = 0;
        let mut append = |text: &str| {
            let part = part_index;
            part_index += 1;
            let mut shared: Option<Arc<str>> = None;
            for found in matcher.find_iter(text) {
                if outcome.results.len() == MAX_SEARCH_RESULTS {
                    outcome.limited = true;
                    return false;
                }
                if shared.is_none() {
                    if text.len() > MAX_SEARCH_TEXT_BYTES - retained {
                        outcome.limited = true;
                        return false;
                    }
                    retained += text.len();
                    shared = Some(Arc::from(text));
                }
                outcome.results.push(SearchResult {
                    element_index,
                    part_index: part,
                    text: Arc::clone(shared.as_ref().unwrap()),
                    start_pos: found.start(),
                    end_pos: found.end(),
                });
            }
            true
        };
        let complete = match element {
            DocumentElement::Heading { text, .. } | DocumentElement::CodeBlock { text } => {
                append(text)
            }
            DocumentElement::Paragraph { runs } => {
                append(&runs.iter().map(|r| r.text.as_str()).collect::<String>())
            }
            DocumentElement::List { items, .. } => items.iter().all(|item| {
                append(
                    &item
                        .runs
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect::<String>(),
                )
            }),
            DocumentElement::Table { table } => {
                (!table.metadata.has_headers
                    || table.headers.iter().all(|cell| append(&cell.content)))
                    && table
                        .rows
                        .iter()
                        .flatten()
                        .all(|cell| append(&cell.content))
            }
            DocumentElement::Image { description, .. } => append(description),
            DocumentElement::Equation { latex, .. } => append(latex),
            DocumentElement::TextBox { lines } => append(&lines.join("\n")),
            DocumentElement::PageBreak => true,
        };
        if !complete {
            break;
        }
    }
    outcome
}

pub fn generate_outline(document: &Document) -> Vec<OutlineItem> {
    let mut outline = Vec::new();

    for (index, element) in document.elements.iter().enumerate() {
        if let DocumentElement::Heading {
            level,
            text,
            number,
        } = element
        {
            let title = if let Some(number) = number {
                format!("{number} {text}")
            } else {
                text.clone()
            };
            outline.push(OutlineItem {
                title,
                level: *level,
                element_index: index,
            });
        }
    }

    outline
}
