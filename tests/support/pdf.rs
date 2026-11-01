//! Original synthetic PDF fixtures. No external document tool is required.

pub struct Page<'a> {
    pub width: u32,
    pub height: u32,
    pub options: &'a str,
    pub content: &'a str,
}

pub fn document(pages: &[Page<'_>]) -> Vec<u8> {
    let kids = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + i * 2))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Count {} /Kids [{kids}] >>", pages.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for (index, page) in pages.iter().enumerate() {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] {} /Resources << /Font << /F1 3 0 R >> /ExtGState << /GS1 << /Type /ExtGState /ca 0.5 >> >> >> /Contents {} 0 R >>",
            page.width, page.height, page.options, 5 + index * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            page.content.len(),
            page.content
        ));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    bytes
}
