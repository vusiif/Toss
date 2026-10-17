//! File I/O operations and validation
//!
//! This module handles file validation and document merge operations.

use anyhow::{bail, Result};
use std::path::Path;

use super::models::DocumentElement;

/// Validates that the file is a legitimate .docx file
pub(crate) fn validate_docx_file(file_path: &Path) -> Result<()> {
    // Check file extension (case-insensitive)
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    if !extension.eq_ignore_ascii_case("docx") {
        bail!(
            "Invalid file format. Expected .docx file, got .{}\n\
            Note: doxx only supports Word .docx files (not .doc, .xlsx, .zip, etc.)",
            extension
        );
    }

    // Full validation using bounded PackageReader
    crate::document::package::DocxPackage::open(file_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

/// Merge display equations into the element list at their correct paragraph positions
///
/// This function handles the fact that docx-rs doesn't parse paragraphs containing only equations.
/// We need to track paragraph indices from the XML and insert equations at the right positions.
pub(crate) fn merge_display_equations(
    elements: Vec<DocumentElement>,
    display_equations_by_para: std::collections::HashMap<usize, Vec<DocumentElement>>,
) -> Vec<DocumentElement> {
    if display_equations_by_para.is_empty() {
        return elements;
    }

    // Get all paragraph indices with equations, sorted
    let mut eq_para_indices: Vec<usize> = display_equations_by_para.keys().copied().collect();
    eq_para_indices.sort_unstable();

    // Build a new element list with equations inserted at correct positions
    let mut result = Vec::new();
    let mut element_para_index = 0;

    for element in elements {
        // Increment paragraph counter for elements that correspond to paragraphs
        match &element {
            DocumentElement::Paragraph { .. }
            | DocumentElement::Heading { .. }
            | DocumentElement::List { .. } => {
                element_para_index += 1;

                // Insert any display equations that come before this element
                while let Some(&eq_idx) = eq_para_indices.first() {
                    if eq_idx < element_para_index {
                        if let Some(eqs) = display_equations_by_para.get(&eq_idx) {
                            result.extend(eqs.clone());
                        }
                        eq_para_indices.remove(0);
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }

        result.push(element);
    }

    // Add any remaining equations at the end
    for eq_idx in eq_para_indices {
        if let Some(eqs) = display_equations_by_para.get(&eq_idx) {
            result.extend(eqs.clone());
        }
    }

    result
}
