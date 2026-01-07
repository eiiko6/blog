use std::fs::File;
use std::path::PathBuf;

// This file is kind of ugly, I tried to make nix build work

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let local_theme_path = PathBuf::from(&manifest_dir).join("themes/Catppuccin-Macchiato.tmTheme");

    let final_path = if let Ok(nix_theme_path) = std::env::var("THEME_PATH") {
        // In case of nix build
        println!("cargo:rerun-if-env-changed=THEME_PATH");
        nix_theme_path
    } else {
        // In case of cargo
        if !local_theme_path.exists() {
            println!("cargo:warning=Theme not found, downloading...");

            std::fs::create_dir_all(local_theme_path.parent().unwrap())
                .expect("Could not create themes directory");

            let mut response = reqwest::blocking::get("https://raw.githubusercontent.com/catppuccin/bat/refs/heads/main/themes/Catppuccin%20Macchiato.tmTheme")
                .expect("Failed to download theme");

            let mut file = File::create(&local_theme_path).expect("Failed to create theme file");
            std::io::copy(&mut response, &mut file).expect("Failed to save theme");
        }

        local_theme_path.to_str().unwrap().to_string()
    };

    println!("cargo:rustc-env=THEME_FILE_PATH={}", final_path);
}
