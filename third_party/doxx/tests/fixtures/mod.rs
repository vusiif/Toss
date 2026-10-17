use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub struct SyntheticDocx {
    document_xml: String,
    include_pkg_rels: bool,
    include_doc_rels: bool,
    numbering_xml: Option<String>,
    custom_pkg_rels: Option<String>,
    custom_doc_rels: Option<String>,
    extra_entries: Vec<(String, Vec<u8>)>,
}

impl SyntheticDocx {
    pub fn new(document_body_xml: &str) -> Self {
        let document_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
    <w:body>
        {document_body_xml}
    </w:body>
</w:document>"#
        );
        Self {
            document_xml,
            include_pkg_rels: true,
            include_doc_rels: true,
            numbering_xml: None,
            custom_pkg_rels: None,
            custom_doc_rels: None,
            extra_entries: Vec::new(),
        }
    }

    /// Omit main document relationships (word/_rels/document.xml.rels)
    #[allow(dead_code)]
    pub fn without_doc_rels(mut self) -> Self {
        self.include_doc_rels = false;
        self
    }

    /// Omit root package relationships (_rels/.rels)
    #[allow(dead_code)]
    pub fn without_pkg_rels(mut self) -> Self {
        self.include_pkg_rels = false;
        self
    }

    #[allow(dead_code)]
    pub fn with_numbering(mut self, numbering_xml: &str) -> Self {
        self.numbering_xml = Some(numbering_xml.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn with_custom_doc_rels(mut self, rels_xml: &str) -> Self {
        self.custom_doc_rels = Some(rels_xml.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn with_custom_pkg_rels(mut self, rels_xml: &str) -> Self {
        self.custom_pkg_rels = Some(rels_xml.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn with_extra_entry(mut self, name: &str, bytes: &[u8]) -> Self {
        self.extra_entries.push((name.to_string(), bytes.to_vec()));
        self
    }

    #[allow(dead_code)]
    pub fn with_image(mut self, rel_id: &str, target_path: &str, image_bytes: &[u8]) -> Self {
        self.extra_entries
            .push((target_path.to_string(), image_bytes.to_vec()));
        let rel_entry = format!(
            r#"    <Relationship Id="{rel_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{target_path}"/>"#
        );
        let updated_rels = match self.custom_doc_rels {
            Some(existing) => existing.replace("</Relationships>", &format!("{rel_entry}\n</Relationships>")),
            None => format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{rel_entry}
</Relationships>"#
            ),
        };
        self.custom_doc_rels = Some(updated_rels);
        self
    }

    pub fn build_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            // [Content_Types].xml
            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="png" ContentType="image/png"/>
    <Default Extension="jpeg" ContentType="image/jpeg"/>
    <Default Extension="jpg" ContentType="image/jpeg"/>
    <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
            )
            .unwrap();

            // _rels/.rels
            if let Some(ref custom_rels) = self.custom_pkg_rels {
                zip.start_file("_rels/.rels", options).unwrap();
                zip.write_all(custom_rels.as_bytes()).unwrap();
            } else if self.include_pkg_rels {
                zip.start_file("_rels/.rels", options).unwrap();
                zip.write_all(
                    br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
                )
                .unwrap();
            }

            // word/_rels/document.xml.rels
            if let Some(ref custom_rels) = self.custom_doc_rels {
                zip.start_file("word/_rels/document.xml.rels", options).unwrap();
                zip.write_all(custom_rels.as_bytes()).unwrap();
            } else if self.include_doc_rels {
                zip.start_file("word/_rels/document.xml.rels", options).unwrap();
                zip.write_all(
                    br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
</Relationships>"#,
                )
                .unwrap();
            }

            // word/document.xml
            zip.start_file("word/document.xml", options).unwrap();
            zip.write_all(self.document_xml.as_bytes()).unwrap();

            // word/numbering.xml (if provided)
            if let Some(num_xml) = &self.numbering_xml {
                zip.start_file("word/numbering.xml", options).unwrap();
                zip.write_all(num_xml.as_bytes()).unwrap();
            }

            // Extra entries (media files, path traversal targets, duplicates, etc.)
            for (entry_name, data) in &self.extra_entries {
                zip.start_file(entry_name, options).unwrap();
                zip.write_all(data).unwrap();
            }

            zip.finish().unwrap();
        }
        buffer
    }
}
