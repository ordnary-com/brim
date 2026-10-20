//! `brim icons`: file icons for `.brim` and `.brimproj` files in the
//! system's file manager (Finder, File Explorer, Linux desktops).
//!
//! Everything is installed for the current user only:
//!
//! - macOS: `~/Applications/Brim.app`, a bundle that declares the file types
//!   and their icons without opening them, registered with Launch Services.
//! - Windows: `.ico` files in `$BRIM_HOME/icons` and the file types under
//!   `HKEY_CURRENT_USER\Software\Classes`.
//! - Linux and other Unix systems: shared MIME types and icons in
//!   `$XDG_DATA_HOME` (`~/.local/share`).
//!
//! The icons are rendered from `assets/icons` by `scripts/icons.sh`.

use std::path::{Path, PathBuf};

use crate::{CmdResult, Context, exit};

/// `brim icons [--remove]`.
pub(crate) fn icons(remove: bool, ctx: &mut Context<'_>) -> CmdResult {
    let result = if remove {
        system::remove()
    } else {
        system::install()
    };
    match result {
        Ok(Some(done)) => {
            let _ = writeln!(ctx.stderr, "{done}");
            Ok(())
        }
        Ok(None) => {
            let _ = writeln!(
                ctx.stderr,
                "error: brim does not know how to install file icons on this system"
            );
            Err(exit::UNAVAILABLE)
        }
        Err(e) => {
            let _ = writeln!(ctx.stderr, "error: {e}");
            Err(exit::INPUT)
        }
    }
}

/// The user's home directory.
#[allow(dead_code)]
fn home() -> std::io::Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("cannot find a home directory; set HOME"))
}

/// Writes `contents` to `path`, creating its directory.
fn write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
        .map_err(|e| std::io::Error::new(e.kind(), format!("cannot write {}: {e}", path.display())))
}

/// Removes a file or directory if it is there.
#[allow(dead_code)]
fn remove_path(path: &Path) -> std::io::Result<()> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(std::io::Error::new(
            e.kind(),
            format!("cannot remove {}: {e}", path.display()),
        )),
        _ => Ok(()),
    }
}

/// Runs a helper program, ignoring whether it exists or succeeds: the
/// files are in place either way, and the system picks them up later.
#[allow(dead_code)]
fn run_quietly(program: &str, args: &[&str]) {
    let _ = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// macOS: a bundle that exports the two file types with their icons.
#[allow(dead_code)]
mod macos {
    use super::*;

    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

    const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.brimlang.icons</string>
  <key>CFBundleName</key><string>Brim</string>
  <key>CFBundleDisplayName</key><string>Brim</string>
  <key>CFBundleExecutable</key><string>brim-icons</string>
  <key>CFBundleIconFile</key><string>app.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSUIElement</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Brim source file</string>
      <key>CFBundleTypeRole</key><string>None</string>
      <key>LSHandlerRank</key><string>None</string>
      <key>CFBundleTypeIconFile</key><string>brim.icns</string>
      <key>LSItemContentTypes</key><array><string>com.brimlang.source</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Brim project</string>
      <key>CFBundleTypeRole</key><string>None</string>
      <key>LSHandlerRank</key><string>None</string>
      <key>CFBundleTypeIconFile</key><string>brimproj.icns</string>
      <key>LSItemContentTypes</key><array><string>com.brimlang.project</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>com.brimlang.source</string>
      <key>UTTypeDescription</key><string>Brim source file</string>
      <key>UTTypeConformsTo</key><array><string>public.source-code</string><string>public.plain-text</string></array>
      <key>UTTypeIconFile</key><string>brim.icns</string>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>brim</string></array></dict>
    </dict>
    <dict>
      <key>UTTypeIdentifier</key><string>com.brimlang.project</string>
      <key>UTTypeDescription</key><string>Brim project</string>
      <key>UTTypeConformsTo</key><array><string>public.plain-text</string></array>
      <key>UTTypeIconFile</key><string>brimproj.icns</string>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>brimproj</string></array></dict>
    </dict>
  </array>
</dict>
</plist>
"#;

    /// The bundle never opens files (its rank is `None`, so files open in
    /// the usual editor); this only runs if someone starts it by hand.
    const EXECUTABLE: &str = "#!/bin/sh\nexit 0\n";

    pub(super) fn bundle(home: &Path) -> PathBuf {
        home.join("Applications").join("Brim.app")
    }

    /// Writes the bundle, replacing an older one.
    pub(super) fn write_bundle(app: &Path) -> std::io::Result<()> {
        remove_path(app)?;
        let contents = app.join("Contents");
        write(&contents.join("Info.plist"), INFO_PLIST.as_bytes())?;
        write(&contents.join("PkgInfo"), b"APPL????")?;
        let resources = contents.join("Resources");
        write(
            &resources.join("brim.icns"),
            include_bytes!("../../../assets/icons/brim.icns"),
        )?;
        write(
            &resources.join("brimproj.icns"),
            include_bytes!("../../../assets/icons/brimproj.icns"),
        )?;
        write(
            &resources.join("app.icns"),
            include_bytes!("../../../assets/icons/app.icns"),
        )?;
        let executable = contents.join("MacOS").join("brim-icons");
        write(&executable, EXECUTABLE.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    }

    pub(super) fn install() -> std::io::Result<String> {
        let app = bundle(&home()?);
        write_bundle(&app)?;
        run_quietly(LSREGISTER, &["-f", &app.to_string_lossy()]);
        Ok(format!(
            "installed file icons for .brim and .brimproj files ({})",
            app.display()
        ))
    }

    pub(super) fn remove() -> std::io::Result<String> {
        let app = bundle(&home()?);
        if app.exists() {
            run_quietly(LSREGISTER, &["-u", &app.to_string_lossy()]);
        }
        remove_path(&app)?;
        Ok("removed the file icons".into())
    }
}

/// Linux and other Unix desktops: shared MIME types with icons.
#[allow(dead_code)]
mod xdg {
    use super::*;

    const MIME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Installed by `brim icons`; `brim icons --remove` removes it. -->
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="text/x-brim">
    <comment>Brim source file</comment>
    <sub-class-of type="text/plain"/>
    <icon name="text-x-brim"/>
    <glob pattern="*.brim"/>
  </mime-type>
  <mime-type type="text/x-brimproj">
    <comment>Brim project</comment>
    <sub-class-of type="text/plain"/>
    <icon name="text-x-brimproj"/>
    <glob pattern=".brimproj"/>
    <glob pattern="*.brimproj"/>
    <glob pattern="brim.toml"/>
  </mime-type>
</mime-info>
"#;

    /// `$XDG_DATA_HOME`, or `~/.local/share`.
    pub(super) fn data_home() -> std::io::Result<PathBuf> {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(dir) if Path::new(&dir).is_absolute() => Ok(PathBuf::from(dir)),
            _ => Ok(home()?.join(".local").join("share")),
        }
    }

    fn files(data: &Path) -> [(PathBuf, &'static [u8]); 3] {
        let icons = data.join("icons/hicolor/scalable/mimetypes");
        [
            (data.join("mime/packages/brim.xml"), MIME_XML.as_bytes()),
            (
                icons.join("text-x-brim.svg"),
                include_bytes!("../../../assets/icons/brim-document.svg"),
            ),
            (
                icons.join("text-x-brimproj.svg"),
                include_bytes!("../../../assets/icons/brimproj-document.svg"),
            ),
        ]
    }

    pub(super) fn write_files(data: &Path) -> std::io::Result<()> {
        for (path, contents) in files(data) {
            write(&path, contents)?;
        }
        Ok(())
    }

    /// Rebuilds the MIME database and icon cache, when their tools exist.
    fn refresh(data: &Path) {
        run_quietly(
            "update-mime-database",
            &[&data.join("mime").to_string_lossy()],
        );
        let hicolor = data.join("icons/hicolor");
        if hicolor.join("index.theme").exists() {
            run_quietly(
                "gtk-update-icon-cache",
                &["-q", "-t", &hicolor.to_string_lossy()],
            );
        }
    }

    pub(super) fn install() -> std::io::Result<String> {
        let data = data_home()?;
        write_files(&data)?;
        refresh(&data);
        Ok("installed file icons for .brim and .brimproj files".into())
    }

    pub(super) fn remove() -> std::io::Result<String> {
        let data = data_home()?;
        for (path, _) in files(&data) {
            remove_path(&path)?;
        }
        refresh(&data);
        Ok("removed the file icons".into())
    }
}

/// Windows: file types under `HKEY_CURRENT_USER\Software\Classes`.
#[allow(dead_code)]
mod windows {
    use super::*;

    const CLASSES: &str = r"HKCU\Software\Classes";

    /// Extension, file type (ProgID), description and icon file.
    pub(super) const TYPES: [(&str, &str, &str, &str); 2] = [
        (".brim", "Brim.Source", "Brim source file", "brim.ico"),
        (".brimproj", "Brim.Project", "Brim project", "brimproj.ico"),
    ];

    pub(super) fn write_icons(dir: &Path) -> std::io::Result<()> {
        write(
            &dir.join("brim.ico"),
            include_bytes!("../../../assets/icons/brim.ico"),
        )?;
        write(
            &dir.join("brimproj.ico"),
            include_bytes!("../../../assets/icons/brimproj.ico"),
        )
    }

    fn icon_dir() -> std::io::Result<PathBuf> {
        brim_package::default_home()
            .map(|home| home.join("icons"))
            .ok_or_else(|| std::io::Error::other("cannot find a home directory; set BRIM_HOME"))
    }

    fn reg(args: &[&str]) -> std::io::Result<bool> {
        let status = std::process::Command::new("reg")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| std::io::Error::new(e.kind(), format!("cannot run reg: {e}")))?;
        Ok(status.success())
    }

    fn reg_set(key: &str, name: Option<&str>, value: &str) -> std::io::Result<()> {
        let mut args = vec!["add", key];
        match name {
            Some(name) => args.extend(["/v", name]),
            None => args.push("/ve"),
        }
        args.extend(["/t", "REG_SZ", "/d", value, "/f"]);
        if reg(&args)? {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "cannot write the registry key {key}"
            )))
        }
    }

    /// Asks Explorer to reload file type icons.
    fn refresh() {
        run_quietly("ie4uinit.exe", &["-show"]);
    }

    pub(super) fn install() -> std::io::Result<String> {
        let dir = icon_dir()?;
        write_icons(&dir)?;
        for (ext, progid, description, icon) in TYPES {
            let class = format!(r"{CLASSES}\{progid}");
            reg_set(&class, None, description)?;
            reg_set(
                &format!(r"{class}\DefaultIcon"),
                None,
                &dir.join(icon).to_string_lossy(),
            )?;
            let key = format!(r"{CLASSES}\{ext}");
            reg_set(&key, None, progid)?;
            reg_set(&key, Some("PerceivedType"), "text")?;
            reg_set(&key, Some("Content Type"), "text/plain")?;
            reg_set(&format!(r"{key}\OpenWithProgids"), Some(progid), "")?;
        }
        refresh();
        Ok(format!(
            "installed file icons for .brim and .brimproj files ({})",
            dir.display()
        ))
    }

    pub(super) fn remove() -> std::io::Result<String> {
        for (ext, progid, _, icon) in TYPES {
            reg(&["delete", &format!(r"{CLASSES}\{progid}"), "/f"])?;
            let key = format!(r"{CLASSES}\{ext}");
            // The extension's key goes only if it still points at Brim's
            // file type; another program may have taken it over.
            let ours = std::process::Command::new("reg")
                .args(["query", &key, "/ve"])
                .output()
                .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(progid));
            if ours {
                reg(&["delete", &key, "/f"])?;
            } else {
                reg(&[
                    "delete",
                    &format!(r"{key}\OpenWithProgids"),
                    "/v",
                    progid,
                    "/f",
                ])?;
            }
            remove_path(&icon_dir()?.join(icon))?;
        }
        refresh();
        Ok("removed the file icons".into())
    }
}

mod system {
    #[allow(unused_imports)]
    use super::*;

    #[cfg(target_os = "macos")]
    pub(super) use super::macos::{install as install_here, remove as remove_here};
    #[cfg(windows)]
    pub(super) use super::windows::{install as install_here, remove as remove_here};
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) use super::xdg::{install as install_here, remove as remove_here};

    #[cfg(any(unix, windows))]
    pub(super) fn install() -> std::io::Result<Option<String>> {
        install_here().map(Some)
    }

    #[cfg(any(unix, windows))]
    pub(super) fn remove() -> std::io::Result<Option<String>> {
        remove_here().map(Some)
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) fn install() -> std::io::Result<Option<String>> {
        Ok(None)
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) fn remove() -> std::io::Result<Option<String>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brim-icons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_macos_bundle_declares_both_file_types() {
        let home = scratch("macos");
        let app = macos::bundle(&home);
        macos::write_bundle(&app).unwrap();
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        for needed in [
            "<string>brim</string>",
            "<string>brimproj</string>",
            "<string>brim.icns</string>",
            "<string>brimproj.icns</string>",
        ] {
            assert!(plist.contains(needed), "Info.plist lacks {needed}");
        }
        // The bundle must never become the program that opens the files.
        assert!(!plist.contains("Owner") && !plist.contains("Viewer"));
        for icon in ["brim.icns", "brimproj.icns", "app.icns"] {
            let data = std::fs::read(app.join("Contents/Resources").join(icon)).unwrap();
            assert_eq!(&data[..4], b"icns");
        }
        // Writing it again replaces it.
        macos::write_bundle(&app).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn the_mime_types_cover_both_file_names() {
        let data = scratch("xdg");
        xdg::write_files(&data).unwrap();
        let xml = std::fs::read_to_string(data.join("mime/packages/brim.xml")).unwrap();
        assert!(xml.contains(r#"<glob pattern="*.brim"/>"#));
        assert!(xml.contains(r#"<glob pattern=".brimproj"/>"#));
        let icons = data.join("icons/hicolor/scalable/mimetypes");
        assert!(icons.join("text-x-brim.svg").is_file());
        assert!(icons.join("text-x-brimproj.svg").is_file());
        std::fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn the_windows_icons_are_ico_files() {
        let dir = scratch("windows");
        windows::write_icons(&dir).unwrap();
        for (_, _, _, icon) in windows::TYPES {
            let data = std::fs::read(dir.join(icon)).unwrap();
            assert_eq!(&data[..4], &[0, 0, 1, 0]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
