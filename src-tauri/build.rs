fn main() {
    tauri_build::build();

    #[cfg(windows)]
    {
        // Keep Tauri's icon and application manifest while completing its VERSIONINFO.
        // Recompile the same resource file so there is only one version resource.
        let resource =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("resource.rc");
        let version = format!("{}.0", env!("CARGO_PKG_VERSION"));
        let original = std::fs::read_to_string(&resource).expect("read Tauri Windows resources");
        let mut lines = Vec::new();
        for line in original.lines() {
            let trimmed = line.trim();
            let replacement = if trimmed.starts_with("BLOCK \"") && trimmed.ends_with("04b0\"") {
                Some("BLOCK \"040904b0\"".to_string())
            } else if trimmed.starts_with("VALUE \"FileVersion\"") {
                Some(format!("VALUE \"FileVersion\", \"{version}\""))
            } else if trimmed.starts_with("VALUE \"ProductVersion\"") {
                Some(format!("VALUE \"ProductVersion\", \"{version}\""))
            } else if trimmed.starts_with("VALUE \"FileDescription\"") {
                Some(format!(
                    "VALUE \"FileDescription\", \"{}\"\nVALUE \"InternalName\", \"CloudDownloader\"\nVALUE \"OriginalFilename\", \"CloudDownloader.exe\"",
                    env!("CARGO_PKG_DESCRIPTION")
                ))
            } else if trimmed.starts_with("VALUE \"Translation\"") {
                Some("VALUE \"Translation\", 0x0409, 0x04b0".to_string())
            } else {
                None
            };
            lines.push(replacement.unwrap_or_else(|| line.to_string()));
        }
        std::fs::write(&resource, lines.join("\n")).expect("write Windows version information");
        // Tauri already emitted the link argument for this file.
        embed_resource::compile_for(&resource, std::iter::empty::<&str>(), embed_resource::NONE)
            .manifest_required()
            .expect("compile Windows version information");
    }
}
