//! Read-only PDF page viewing; platform capabilities own the window.

use std::path::Path;

use crate::core::error::TossError;

pub fn view(path: &Path) -> Result<(), TossError> {
    let metadata = std::fs::metadata(path).map_err(|err| TossError::from_io(path, err))?;
    if !metadata.is_file() {
        return Err(TossError::invalid_arguments(format!(
            "{} is not a PDF file",
            path.display()
        )));
    }
    crate::platform::pdf::view(path)
}
