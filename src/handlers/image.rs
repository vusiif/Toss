//! What Toss does with an input it classified as an image.
//!
//! §18.3's minimum is open, zoom, pan and previous/next, and only the first of
//! those needs a window. This module owns the half that is identical on every
//! platform: which images belong to the session, which one is open, and the
//! order they come in (IMAGE_VIEWER.md §4). The window, the decode and the
//! drawing are the platform's business and never appear here.

use std::path::{Path, PathBuf};

use crate::core::error::TossError;
use crate::core::input::Input;
use crate::detection::{self, Kind};
use crate::platform::image::ViewRequest;

/// Open `image`, with its siblings ready for previous and next.
///
/// The set and the index are decided before the request is handed over, so a
/// platform backend only has to answer "show this one, here are its
/// neighbours" — it never has to decide what a neighbour is.
pub fn view(image: &Path) -> Result<(), TossError> {
    // `toss view Documents/` names a directory, not a picture. Saying so is
    // more use than letting the platform half discover it later (§4.2, §23).
    if !image.is_file() {
        return Err(TossError::invalid_arguments(format!(
            "{} is not an image file",
            image.display()
        )));
    }

    let (images, index) = image_set(image);

    crate::platform::image::view(&ViewRequest { images, index })
}

/// The images of one browsing session: everything that is an image beside
/// `image`, in name order, with `image`'s own position.
///
/// The position is found by file name rather than by path because the two do
/// not have the same shape: `toss photo.jpg` arrives as `photo.jpg` while
/// `read_dir` reports `./photo.jpg`, and comparing the strings would report
/// every input as missing from its own directory.
fn image_set(image: &Path) -> (Vec<PathBuf>, usize) {
    let mut images = siblings(image);

    let index = match images
        .iter()
        .position(|entry| entry.file_name() == image.file_name())
    {
        Some(found) => found,
        // Detected by its contents rather than by a suffix Toss recognises,
        // or simply absent from the listing: it still belongs in the set, or
        // the window would open on somebody else's picture. Inserted in order
        // so that previous/next stays in name order for everybody else.
        None => {
            let position = images.partition_point(|entry| entry.file_name() < image.file_name());
            images.insert(position, image.to_path_buf());
            position
        }
    };

    (images, index)
}

/// Every image in the same directory as `image`, in name order.
///
/// Classification is delegated to [`detection`] rather than re-derived from
/// the extension here, so that one file is called an image by one rule
/// wherever it is met (§6). What that costs is one bounded header read per
/// file with an unrecognised suffix — never a whole file, which is the thing
/// §35 rules out.
fn siblings(image: &Path) -> Vec<PathBuf> {
    let directory = match image.parent() {
        // `Path::new("photo.jpg").parent()` is the empty path, not the current
        // directory, and `read_dir("")` fails — so an input given as a bare
        // file name has to be pointed at `.` explicitly.
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| detection::classify(&Input::File(path.clone())) == Kind::Image)
        .collect();

    // Name order rather than directory order: `read_dir` sequences differ
    // between filesystems, and the browsing order has to be the same
    // everywhere the same folder is opened (§22).
    found.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    found
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::image_set;
    use crate::core::error::TossError;
    use crate::handlers::image::view;

    /// A scratch directory that cleans itself up, so a failing test does not
    /// leave pictures behind for the next run to trip over.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("toss-image-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch directory");
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

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const JPEG: &[u8] = b"\xFF\xD8\xFF";

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("file written");
    }

    fn names(images: &[PathBuf]) -> Vec<String> {
        images
            .iter()
            .map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn the_set_is_the_images_beside_this_one_in_name_order() {
        let scratch = Scratch::new("set");

        write(&scratch.path().join("b.png"), PNG);
        write(&scratch.path().join("a.jpg"), JPEG);
        write(&scratch.path().join("01.png"), PNG);
        write(&scratch.path().join("notes.txt"), b"not a picture\n");
        std::fs::create_dir_all(scratch.path().join("sub")).expect("subdirectory");

        let (images, index) = image_set(&scratch.path().join("b.png"));

        // The `.txt` and the subdirectory are both out; `01` sorts before
        // `a` because the order is byte order, not the order the directory
        // happened to hand them back in.
        assert_eq!(names(&images), ["01.png", "a.jpg", "b.png"]);
        assert_eq!(
            images.get(index).and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("b.png")),
            "the index has to name the picture that was asked for"
        );
    }

    #[test]
    fn an_image_detected_only_by_its_contents_joins_the_set_in_order() {
        // No extension for `detection` to read, so it is identified by its
        // header — which is exactly the case where the file name will not be
        // found among the siblings that have one.
        let scratch = Scratch::new("magic");
        write(&scratch.path().join("cover.png"), PNG);
        write(&scratch.path().join("photo"), PNG);

        let (images, index) = image_set(&scratch.path().join("photo"));

        assert_eq!(names(&images), ["cover.png", "photo"]);
        assert_eq!(
            index, 1,
            "it is inserted rather than appended, so the order survives"
        );
    }

    #[test]
    fn a_lone_image_is_a_session_of_one() {
        // The 0/1/N boundary from IMAGE_VIEWER.md §5, at the set level: one
        // image has to be both the first and the only thing prev/next can
        // reach, and this is what makes that true.
        let scratch = Scratch::new("lone");
        write(&scratch.path().join("only.png"), PNG);

        let (images, index) = image_set(&scratch.path().join("only.png"));

        assert_eq!(images.len(), 1);
        assert_eq!(index, 0);
    }

    #[test]
    fn non_ascii_names_keep_their_place_in_the_order() {
        let scratch = Scratch::new("unicode");
        write(&scratch.path().join("中文.png"), PNG);
        write(&scratch.path().join("z.png"), PNG);

        let (images, index) = image_set(&scratch.path().join("中文.png"));

        assert_eq!(names(&images), ["z.png", "中文.png"]);
        assert_eq!(index, 1);
    }

    #[test]
    fn a_directory_is_refused_before_any_platform_work_is_asked_for() {
        let scratch = Scratch::new("not-a-file");
        std::fs::create_dir_all(scratch.path().join("folder")).expect("directory");

        let err = view(&scratch.path().join("folder")).expect_err("a folder is not a picture");

        assert!(
            matches!(err, TossError::InvalidArguments(_)),
            "expected a refusal, got {err:?}"
        );
        assert!(
            err.to_string().contains("is not an image file"),
            "the message must name the problem, got: {err}"
        );
    }
}
