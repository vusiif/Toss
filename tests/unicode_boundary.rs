//! Unicode and special character path boundary tests (§22, §23, §24).
//!
//! Validates that paths containing spaces, non-ASCII characters (CJK, accents),
//! and multi-byte UTF-8 emojis:
//!
//! 1. Pass through CLI parsing without truncation or encoding loss.
//! 2. Fallback to file facts inspection with exit code 0 when unhandled.
//! 3. Correctly report not-found with exit code 4 and preserve the exact input path.
//! 4. Cleanly refuse mismatched verbs with exit codes 2 or 3 and clear diagnostics.
//! 5. Distinguish corrupt inputs with exit code 6 when backends are enabled.
//!
//! No GUI window is opened by any test in this file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary directory that cleans itself up on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(suffix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("toss-unicode-test-{}-{suffix}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory creation");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the shipped `toss` binary with the given arguments.
fn toss(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss binary runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn unknown_file_with_unicode_and_spaces_falls_back_to_info() {
    let scratch = Scratch::new("info-fallback");
    let file_name = "测试 备忘 报告 😊.unknown";
    let target = scratch.path().join(file_name);
    std::fs::write(&target, b"Arbitrary payload contents").expect("sample written");

    let output = toss(&[target.as_path()]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "unhandled file should fall back to info reporter with exit code 0: {}",
        stderr(&output)
    );
    assert!(
        output.stderr.is_empty(),
        "fallback info should not emit errors to stderr: {}",
        stderr(&output)
    );

    let out = stdout(&output);
    assert!(
        out.contains(&format!("Filename     : {file_name}")),
        "stdout should report the verbatim unicode filename: {out}"
    );
    assert!(
        out.contains("Extension    : unknown"),
        "stdout should report the extension correctly: {out}"
    );
    assert!(
        out.contains("Detected type: unknown"),
        "stdout should report unknown detected type: {out}"
    );
    assert!(
        out.contains("26 bytes"),
        "stdout should report exact byte length: {out}"
    );
}

#[test]
fn missing_path_with_unicode_and_spaces_reports_not_found() {
    let scratch = Scratch::new("not-found");
    let missing_name = "不存在的 文件 😊.xyz";
    let missing = scratch.path().join(missing_name);

    let output = toss(&[missing.as_path()]);

    assert_eq!(
        output.status.code(),
        Some(4),
        "missing path must return exit code 4: {}",
        stderr(&output)
    );
    let err = stderr(&output);
    assert!(
        err.starts_with("Error: "),
        "error message must begin with 'Error: ': {err}"
    );
    assert!(
        err.contains(missing_name),
        "error diagnostic must preserve the full unicode path: {err}"
    );
}

#[test]
fn verb_type_mismatch_with_unicode_paths_refuses_cleanly() {
    let scratch = Scratch::new("mismatch");
    let file_name = "文档 资料 😊.txt";
    let file_path = scratch.path().join(file_name);
    std::fs::write(&file_path, b"plain text data").expect("file written");

    let dir_name = "分类 目录 😊";
    let dir_path = scratch.path().join(dir_name);
    std::fs::create_dir_all(&dir_path).expect("directory created");

    // 1. `toss view <text_file>` -> Exit 3 (Unsupported format for viewer)
    let view_txt = toss(&[Path::new("view"), file_path.as_path()]);
    assert_eq!(
        view_txt.status.code(),
        Some(3),
        "viewing text file must return exit code 3: {}",
        stderr(&view_txt)
    );

    // 2. `toss view <directory>` -> Exit 2 (Invalid arguments, directory is not an image)
    let view_dir = toss(&[Path::new("view"), dir_path.as_path()]);
    assert_eq!(
        view_dir.status.code(),
        Some(2),
        "viewing directory must return exit code 2: {}",
        stderr(&view_dir)
    );
    let err_dir = stderr(&view_dir);
    assert!(
        err_dir.contains(dir_name) && err_dir.contains("is not an image file"),
        "diagnostic must explain directory is not an image: {err_dir}"
    );

    // 3. `toss play <text_file>` -> Exit 3 (Media player refuses non-media)
    let play_txt = toss(&[Path::new("play"), file_path.as_path()]);
    assert_eq!(
        play_txt.status.code(),
        Some(3),
        "playing text file must return exit code 3: {}",
        stderr(&play_txt)
    );

    // 4. `toss play <directory>` -> Exit 2 (Invalid arguments, directory is not media)
    let play_dir = toss(&[Path::new("play"), dir_path.as_path()]);
    assert_eq!(
        play_dir.status.code(),
        Some(2),
        "playing directory must return exit code 2: {}",
        stderr(&play_dir)
    );
    let err_play_dir = stderr(&play_dir);
    assert!(
        err_play_dir.contains(dir_name) && err_play_dir.contains("is not a media file"),
        "diagnostic must explain directory is not a media file: {err_play_dir}"
    );

    // 5. `toss pack <file>` -> Exit 2 (Invalid arguments, file is not a directory)
    let pack_file = toss(&[Path::new("pack"), file_path.as_path()]);
    assert_eq!(
        pack_file.status.code(),
        Some(2),
        "packing a regular file must return exit code 2: {}",
        stderr(&pack_file)
    );
    let err_pack = stderr(&pack_file);
    assert!(
        err_pack.contains(file_name) && err_pack.contains("is not a directory"),
        "diagnostic must explain path is not a directory: {err_pack}"
    );
}

#[test]
fn corrupt_media_and_image_with_unicode_names_reports_corrupt_input() {
    let scratch = Scratch::new("corrupt-unicode");

    // Copy corrupt image fixture
    let corrupt_img_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("image")
        .join("truncated.png");
    let corrupt_img_dst = scratch.path().join("损坏 图像 样例 😊.png");
    std::fs::copy(&corrupt_img_src, &corrupt_img_dst).expect("copied corrupt image");

    let out_img = toss(&[corrupt_img_dst.as_path()]);
    if cfg!(all(windows, feature = "image")) {
        assert_eq!(
            out_img.status.code(),
            Some(6),
            "corrupt image should yield exit code 6: {}",
            stderr(&out_img)
        );
        assert!(
            stderr(&out_img).contains("损坏 图像 样例 😊.png"),
            "diagnostic must name the corrupt image: {}",
            stderr(&out_img)
        );
    } else {
        assert_eq!(
            out_img.status.code(),
            Some(3),
            "unsupported image feature should yield exit code 3: {}",
            stderr(&out_img)
        );
        assert!(
            stderr(&out_img).contains("not implemented yet: image viewing"),
            "diagnostic must report unimplemented feature: {}",
            stderr(&out_img)
        );
    }

    // Copy corrupt media fixture
    let corrupt_media_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("media")
        .join("truncated.mp4");
    let corrupt_media_dst = scratch.path().join("损坏 视频 样例 😊.mp4");
    std::fs::copy(&corrupt_media_src, &corrupt_media_dst).expect("copied corrupt media");

    let out_media = toss(&[corrupt_media_dst.as_path()]);
    if cfg!(all(windows, feature = "media")) {
        assert_eq!(
            out_media.status.code(),
            Some(6),
            "corrupt media should yield exit code 6: {}",
            stderr(&out_media)
        );
        assert!(
            stderr(&out_media).contains("损坏 视频 样例 😊.mp4"),
            "diagnostic must name the corrupt media: {}",
            stderr(&out_media)
        );
    } else {
        assert_eq!(
            out_media.status.code(),
            Some(3),
            "unsupported media feature should yield exit code 3: {}",
            stderr(&out_media)
        );
        assert!(
            stderr(&out_media).contains("not implemented yet: media playback"),
            "diagnostic must report unimplemented feature: {}",
            stderr(&out_media)
        );
    }
}

#[test]
fn multiple_unicode_inputs_handled_in_sequence() {
    let scratch = Scratch::new("multi-unicode");
    let f1 = scratch.path().join("第一 报告 😊.bin");
    let f2 = scratch.path().join("第二 报告 😊.bin");
    std::fs::write(&f1, b"first payload").expect("f1 written");
    std::fs::write(&f2, b"second payload bytes").expect("f2 written");

    let output = toss(&[f1.as_path(), f2.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "multiple info reports should all succeed with exit code 0: {}",
        stderr(&output)
    );

    let out = stdout(&output);
    assert!(
        out.contains("第一 报告 😊.bin") && out.contains("第二 报告 😊.bin"),
        "both files must be described in stdout: {out}"
    );
}
