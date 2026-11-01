use std::io::{Cursor, Read, Write};

/// Build a real DOC with a single UTF-16 piece, CHPX FKP image runs and PICF
/// records in the Data stream. Format structures follow MS-DOC, not viewer mocks.
#[allow(dead_code)]
pub fn images_doc(images: &[Vec<u8>]) -> Vec<u8> {
    assert!(images.len() <= 8);
    let mut text = String::from("BEFORE\r");
    for index in 0..images.len() {
        text.push_str(&format!("\u{1}AFTER_{index}\r"));
    }
    let bytes = legacy_doc(&text, false, false);
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes)).unwrap();
    let mut word = Vec::new();
    compound
        .open_stream("WordDocument")
        .unwrap()
        .read_to_end(&mut word)
        .unwrap();
    let mut table = Vec::new();
    compound
        .open_stream("0Table")
        .unwrap()
        .read_to_end(&mut table)
        .unwrap();
    let bte_offset = table.len() as u32;
    let text_end = word.len() as u32;
    table.extend(1024u32.to_le_bytes());
    table.extend(text_end.to_le_bytes());
    table.extend(8u32.to_le_bytes()); // FKP at WordDocument page 8.
    word[250..254].copy_from_slice(&bte_offset.to_le_bytes());
    word[254..258].copy_from_slice(&12u32.to_le_bytes());
    let mut boundaries = vec![1024u32];
    let mut image_offsets = Vec::new();
    let mut fc = 1024u32;
    let mut data = Vec::new();
    for (cp, ch) in text.encode_utf16().enumerate() {
        if ch == 1 {
            boundaries.push(fc);
            boundaries.push(fc + 2);
            image_offsets.push((boundaries.len() - 2, data.len() as u32));
            let payload = &images[image_offsets.len() - 1];
            data.extend(((payload.len() + 8) as u32).to_le_bytes());
            data.extend(8u16.to_le_bytes());
            data.extend(0u16.to_le_bytes());
            data.extend(payload);
        }
        fc = 1024 + ((cp + 1) * 2) as u32;
    }
    boundaries.push(text_end);
    let count = boundaries.len() - 1;
    let mut fkp = [0u8; 512];
    fkp[511] = count as u8;
    for (i, boundary) in boundaries.iter().enumerate() {
        fkp[i * 4..i * 4 + 4].copy_from_slice(&boundary.to_le_bytes());
    }
    for (ordinal, (run_index, offset)) in image_offsets.iter().enumerate() {
        let position = 200 + ordinal * 16;
        fkp[boundaries.len() * 4 + run_index] = (position / 2) as u8;
        fkp[position] = 9;
        fkp[position + 1..position + 4].copy_from_slice(&[0x55, 0x08, 1]); // fSpec
        fkp[position + 4..position + 6].copy_from_slice(&[0x03, 0x6a]); // fcPic
        fkp[position + 6..position + 10].copy_from_slice(&offset.to_le_bytes());
    }
    word.resize(4096, 0);
    word.extend(fkp);
    compound
        .create_stream("WordDocument")
        .unwrap()
        .write_all(&word)
        .unwrap();
    compound
        .create_stream("0Table")
        .unwrap()
        .write_all(&table)
        .unwrap();
    compound
        .create_stream("Data")
        .unwrap()
        .write_all(&data)
        .unwrap();
    compound.into_inner().into_inner()
}

#[allow(dead_code)]
pub fn text_box_doc() -> Vec<u8> {
    let bytes = legacy_doc("MAIN_BODY\rTEXT_BOX_ONLY\r", false, false);
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes)).unwrap();
    let mut word = Vec::new();
    compound
        .open_stream("WordDocument")
        .unwrap()
        .read_to_end(&mut word)
        .unwrap();
    word[76..80].copy_from_slice(&10u32.to_le_bytes());
    word[100..104].copy_from_slice(&14u32.to_le_bytes());
    compound
        .create_stream("WordDocument")
        .unwrap()
        .write_all(&word)
        .unwrap();
    compound.into_inner().into_inner()
}

pub fn legacy_doc(text: &str, compressed: bool, encrypted: bool) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut word = vec![0u8; 1024];
    word[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
    word[2..4].copy_from_slice(&0xC1u16.to_le_bytes());
    word[6..8].copy_from_slice(&0x0409u16.to_le_bytes());
    word[10..12].copy_from_slice(&(if encrypted { 0x100u16 } else { 0 }).to_le_bytes());
    word[32..34].copy_from_slice(&14u16.to_le_bytes());
    word[62..64].copy_from_slice(&22u16.to_le_bytes());
    word[76..80].copy_from_slice(&(units.len() as u32).to_le_bytes());
    word[152..154].copy_from_slice(&93u16.to_le_bytes());
    word[422..426].copy_from_slice(&21u32.to_le_bytes()); // lcbClx, pair 33
    if compressed {
        word.extend(text.bytes());
    } else {
        for u in &units {
            word.extend(u.to_le_bytes());
        }
    }
    let mut table = vec![2];
    table.extend(16u32.to_le_bytes());
    table.extend(0u32.to_le_bytes());
    table.extend((units.len() as u32).to_le_bytes());
    table.extend(0u16.to_le_bytes());
    let fc = if compressed {
        0x4000_0000u32 | 2048
    } else {
        1024
    };
    table.extend(fc.to_le_bytes());
    table.extend(0u16.to_le_bytes());
    let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    cfb.create_stream("/WordDocument")
        .unwrap()
        .write_all(&word)
        .unwrap();
    cfb.create_stream("/0Table")
        .unwrap()
        .write_all(&table)
        .unwrap();
    cfb.into_inner().into_inner()
}
