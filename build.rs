use std::fs::{File, create_dir};
use std::path::Path;

fn main() {
    let dest_path = Path::new("themes/Catppuccin-Macchiato.tmTheme");

    if !dest_path.exists() {
        let mut response = reqwest::blocking::get("https://raw.githubusercontent.com/catppuccin/bat/refs/heads/main/themes/Catppuccin%20Macchiato.tmTheme")
            .expect("Failed to download theme");

        create_dir("themes").expect("Could not create themes dir");
        let mut file = File::create(dest_path).expect("Failed to create theme file");
        std::io::copy(&mut response, &mut file).expect("Failed to save theme");
    }
}
