# Upstream Provenance: doxx 0.1.4

## Provenance

- **Crate**: `doxx`
- **Version**: `0.1.4`
- **Release Source**: [crates.io/crates/doxx/0.1.4](https://crates.io/crates/doxx/0.1.4)
- **Repository**: [https://github.com/bgreenwell/doxx](https://github.com/bgreenwell/doxx)
- **Author**: Brandon Greenwell <greenwell.brandon@gmail.com>
- **License**: MIT (see [LICENSE](LICENSE))
- **SHA-256 of `.crate` tarball**: `5EB8C750F34BB9A8756FEA542618521E8CF0138681F50E9073ABBD1A24F4EA6B`
- **Underlying Parser**: `docx-rs 0.4.18` (SHA-256 `3F21BE13B97BD2924F30323D674F5A8DB382964972825ABD93F30D08F21DAD98`)

## Role in Toss

`doxx` provides the terminal document viewer, document model, and Word XML parsing adaptation for `.docx` files when the optional `word` feature is enabled.

## Adaptations and Patches for Toss (Phase A)

1. **Search Input Key Handling**:
   - In upstream `0.1.4`, global keybindings (`n`, `p`, `q`, `o`, `s`, `h`, `c`) intercepted keystrokes while in search mode (e.g. typing `revenue` dropped `n` as next match or typing `q` exited the viewer).
   - In this adaptation, search mode processes character input first. Typing letters never triggers global commands. Esc cancels search and returns to document view; Enter confirms query and navigates to the first match; Backspace edits query; empty search query is handled cleanly without panics.
2. **Nested Table Preservation**:
   - In upstream `0.1.4` (`src/document/parsing/table.rs`), `TableCellContent::Table` and structured data were ignored with `_ => {}`, causing inner table content to be silently lost.
   - In this adaptation, nested tables and content containers are recursively extracted and formatted into the cell, preserving all inner text and data.
3. **Tab and Line Break Distinction**:
   - In upstream `0.1.4`, `<w:tab/>` was ignored during run extraction in `loader.rs` and `table.rs`, while `<w:br/>` produced `\n` that was treated as a zero-width grapheme in `wrap_runs`, resulting in runs concatenating into words like `BREAK_ABREAK_BBREAK_C`.
   - In this adaptation, `<w:tab/>` produces `\t` and is expanded to tab stops in layout, and `<w:br/>` ends the current visual line and starts a new line.
4. **Table Cell Layout and Unicode Safety**:
   - In upstream `0.1.4`, table cells were truncated using byte-indexing (`&cell.content[..width - 1]`), causing panics on multi-byte UTF-8 boundaries (e.g. emojis `🙂`) and permanently hiding long text with `…`.
   - In this adaptation, cell widths are calculated safely using grapheme clusters, multi-line cell wrapping is supported so full text is readable, and empty cells maintain column alignment.
5. **Decoupled Library Interface & Pure State Transitions**:
   - Upstream `ui.rs` and `state.rs` were tied to binary `Cli`, persistent JSON config/history directories (`~/.config/doxx`), clipboard (`arboard`), and `tokio`.
   - In this adaptation, the viewer operates in-memory with zero disk configuration, exposes pure methods (`App::handle_key_event` and `render_element_to_text`) for deterministic headless testing, and eliminates unused heavy dependencies.

## Adaptations and Patches for Toss (Phase B)

6. **Bounded Read-Only Package Reader (`DocxPackage`)**:
   - Replaced unconstrained ZIP access with streaming decompression budget accounting (`read_bounded`), limiting entries (10,000), `document.xml` (16 MiB), metadata parts (2 MiB), single images (20 MiB), and total uncompressed size (64 MiB).
   - Enforced archive security: path traversal blocking (`..`, leading slashes, Windows drive letters), rejection of encrypted archives (`file.encrypted()`), rejection of duplicate main document parts, and safe handling of external relationships (`TargetMode="External"`: no network calls, sandboxed).
   - In-memory synthesis of `document.xml.rels` when omitted, preventing docx-rs crashes.
7. **Per-Session RAII Temporary Directory (`ImageExtractor`)**:
   - Replaced upstream shared temporary directory (`std::env::temp_dir().join("doxx_images")`) with an isolated per-session `tempfile::TempDir` with prefix `toss_docx_`.
   - Tied image session lifetime directly to the `Document`'s lifetime via `image_session: Option<Arc<ImageExtractor>>` in `Document`, ensuring automatic cleanup on drop and zero collision between concurrent document sessions.
8. **Stable Image Association Across Scrolling**:
   - Fixed upstream bug where `protocol_idx` was reset to 0 for the visible elements loop in `DocumentWidget::render`, which caused images to swap and shift indices whenever the user scrolled down past earlier images.
   - Associated `image_protocols` strictly by immutable `element_index` (`HashMap<usize, StatefulProtocol>`), guaranteeing 1-to-1 stability after scrolling down and back up.
