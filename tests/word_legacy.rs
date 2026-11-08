//! Real CFB/FIB/CLX fixtures exercise the public CLI, not a text-parser mock.
#![cfg(feature = "word")]

use std::io::Cursor;
use std::process::Command;

#[path = "support/legacy_doc.rs"]
mod fixtures;
use fixtures::legacy_doc;

#[test]
fn legacy_doc_unicode_and_compressed_text_through_auto_and_view() {
    let temp = tempfile_for_test("unicode");
    for (name, text, compressed) in [
        ("中文 空格.DOC", "正文中文🙂\rSECOND\tCOLUMN\r", false),
        ("western.doc", "FIRST paragraph\rSECOND paragraph\r", true),
    ] {
        let path = temp.join(name);
        let bytes = legacy_doc(text, compressed, false);
        std::fs::write(&path, &bytes).unwrap();
        for verb in [None, Some("view")] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_toss"));
            if let Some(verb) = verb {
                cmd.arg(verb);
            }
            let out = cmd.arg(&path).output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8(out.stdout).unwrap();
            for word in text.split_whitespace() {
                assert!(stdout.contains(word), "missing {word}: {stdout}");
            }
            assert!(!stdout.contains('\x1b'));
        }
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn legacy_doc_rejects_corrupt_encrypted_and_missing_streams() {
    let temp = tempfile_for_test("errors");
    let empty_cfb = cfb::CompoundFile::create(Cursor::new(Vec::new()))
        .unwrap()
        .into_inner()
        .into_inner();
    for (name, bytes) in [
        ("garbage.doc", b"not a Word document".to_vec()),
        ("streams.doc", empty_cfb),
        ("encrypted.doc", legacy_doc("SECRET\r", false, true)),
    ] {
        let path = temp.join(name);
        std::fs::write(&path, bytes).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_toss"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    }
    let out = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg(temp.join("missing.doc"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    std::fs::remove_dir_all(temp).unwrap();
}

fn tempfile_for_test(name: &str) -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".sandbox-test/word-direct-20261027/tests");
    let path = root.join(format!("{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn libreoffice_generated_doc_preserves_body_and_nested_cells() {
    for (name, markers) in [
        (
            "nested_tables.doc",
            vec!["Outer cell text", "Inner cell text"],
        ),
        (
            "floating_text_bearing.doc",
            vec!["Containing block before.", "Containing block after."],
        ),
        (
            "floating_wrap_policy.doc",
            vec!["Wrap before.", "Wrap after."],
        ),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/word-legacy")
            .join(name);
        let before = std::fs::read(&path).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_toss"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        for marker in markers {
            assert!(text.contains(marker), "{name}: missing {marker}: {text}");
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn legacy_doc_picture_and_text_box_content_through_cli() {
    let temp = tempfile_for_test("pictures-textboxes");
    let mut png = Vec::new();
    doxx::image::RgbaImage::from_pixel(4, 4, doxx::image::Rgba([255, 0, 0, 255]))
        .write_to(&mut Cursor::new(&mut png), doxx::image::ImageFormat::Png)
        .unwrap();
    let inputs = [
        (
            "pictures.doc",
            fixtures::images_doc(&[png]),
            vec!["BEFORE", "[Image]", "AFTER_0"],
        ),
        (
            "textbox.doc",
            fixtures::text_box_doc(),
            vec!["MAIN_BODY", "[Text boxes]", "TEXT_BOX_ONLY"],
        ),
    ];
    for (name, bytes, markers) in inputs {
        let path = temp.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_toss"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        for marker in markers {
            assert_eq!(text.matches(marker).count(), 1, "{name}: {text}");
        }
        assert!(!text.contains('\x1b'));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn redirected_doc_images_need_no_writable_temp_directory() {
    let temp = tempfile_for_test("text-no-temp");
    let path = temp.join("pictures.doc");
    // The shared fixture constructor builds a genuine CFB/Word picture stream.
    let png = doxx::image::RgbaImage::from_pixel(2, 2, doxx::image::Rgba([1, 2, 3, 255]));
    let mut encoded = Vec::new();
    png.write_to(
        &mut Cursor::new(&mut encoded),
        doxx::image::ImageFormat::Png,
    )
    .unwrap();
    std::fs::write(&path, fixtures::images_doc(&[encoded])).unwrap();
    let unavailable = temp.join("absent").join("temp");
    let output = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg(&path)
        .env("TMP", &unavailable)
        .env("TEMP", &unavailable)
        .env("TMPDIR", &unavailable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("[Image]") && text.contains("BEFORE") && text.contains("AFTER_0"));
    assert!(!unavailable.exists());
    std::fs::remove_dir_all(temp).unwrap();
}
