use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::{LogicalSize, WebviewWindow};

const FILE_NAME: &str = "window-size.json";
const MIN_WIDTH: f64 = 720.0;
const MIN_HEIGHT: f64 = 520.0;
const MAX_WIDTH: f64 = 7_680.0;
const MAX_HEIGHT: f64 = 4_320.0;

#[derive(Debug, Deserialize, Serialize)]
struct WindowSize {
    width: f64,
    height: f64,
}

impl WindowSize {
    fn valid(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && (MIN_WIDTH..=MAX_WIDTH).contains(&self.width)
            && (MIN_HEIGHT..=MAX_HEIGHT).contains(&self.height)
    }
}

pub(crate) fn restore(window: &WebviewWindow, data_directory: &Path) {
    let Ok(text) = fs::read_to_string(data_directory.join(FILE_NAME)) else {
        return;
    };
    let Ok(size) = serde_json::from_str::<WindowSize>(&text) else {
        return;
    };
    if size.valid() {
        let _ = window.set_size(LogicalSize::new(size.width, size.height));
    }
}

pub(crate) fn save(window: &tauri::Window, data_directory: &Path) {
    let (Ok(physical), Ok(scale_factor)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let logical = physical.to_logical::<f64>(scale_factor);
    let size = WindowSize {
        width: logical.width,
        height: logical.height,
    };
    if !size.valid() || fs::create_dir_all(data_directory).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(&size) {
        let _ = fs::write(data_directory.join(FILE_NAME), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_sizes_outside_supported_window_bounds() {
        assert!(WindowSize {
            width: 1_280.0,
            height: 800.0
        }
        .valid());
        assert!(!WindowSize {
            width: 640.0,
            height: 800.0
        }
        .valid());
        assert!(!WindowSize {
            width: f64::NAN,
            height: 800.0
        }
        .valid());
    }
}
