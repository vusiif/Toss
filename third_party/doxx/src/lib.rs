//! doxx: Terminal document viewer for .docx files
//!
//! This library provides functionality for parsing Microsoft Word documents
//! and displaying them in terminal environments with rich formatting support.

pub mod ansi;
pub mod config;
pub mod document;
pub mod equation;
pub mod export;
pub mod image_extractor;
pub mod keymap;
pub mod state;
pub mod terminal_image;
pub mod ui;
pub mod widgets;

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Text,
    Csv,
    Json,
    Ansi,
}

/// Color depth options for ANSI export
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// Auto-detect terminal color capabilities
    Auto,
    /// Monochrome (no colors)
    Monochrome,
    /// 16 colors
    Standard,
    /// 256 colors
    Extended,
    /// 24-bit true color
    TrueColor,
}

// Re-export commonly used types
pub use crossterm;
pub use document::{Document, DocumentElement};
pub use equation::Equation;
pub use image;
pub use image_extractor::ImageExtractor;
pub use tempfile;
pub use terminal_image::{TerminalImageRenderer, TerminalImageSupport};
pub use ui::{print_document_plain, run_toss_viewer, App, AppOptions, TerminalGuard, ViewMode};
pub use zip;

