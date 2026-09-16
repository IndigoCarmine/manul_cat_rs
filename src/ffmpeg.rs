//! Finding `ffmpeg`, and what to do when it is not there.
//!
//! Manul does not ship an encoder. Video export renders PNG frames itself and
//! hands them to an `ffmpeg` that lives on the user's machine, which keeps the
//! `.deb` / `.msi` / `.app` installers free of a ~100 MB native dependency and
//! of ffmpeg's licensing obligations.
//!
//! When it is missing, the route back depends on the platform. Windows has
//! `winget` built in, so the app can offer to run one command for the user;
//! everywhere else the package manager is the user's business and this module
//! only supplies the command to type. Nothing here downloads a binary or runs
//! anything without the caller having asked a human first — see
//! [`InstallRoute::Managed`].

use std::path::{Path, PathBuf};
use std::process::Command;

/// Frame file names handed to ffmpeg, as a printf pattern and a formatter.
///
/// Six digits covers 999,999 frames; a trajectory that long is already far past
/// what fits in memory.
pub const FRAME_PATTERN: &str = "frame_%06d.png";

pub fn frame_file_name(index: usize) -> String {
    format!("frame_{index:06}.png")
}

/// Whether `name` is one of [`frame_file_name`]'s outputs.
///
/// Used to clear a frame directory without touching anything that is not ours:
/// the directory sits next to a user-chosen destination.
pub fn is_frame_file_name(name: &str) -> bool {
    let Some(digits) = name
        .strip_prefix("frame_")
        .and_then(|rest| rest.strip_suffix(".png"))
    else {
        return false;
    };
    // `frame_file_name` zero-pads to six, but a trajectory past 999,999 frames
    // would widen it, and those files are ours too.
    digits.len() >= 6 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// How this platform expects ffmpeg to be installed.
pub enum InstallRoute {
    /// A single command the app can run on the user's behalf once they agree.
    /// The exact command is shown in the consent dialog, so nothing runs that
    /// the user has not read.
    Managed {
        /// What to call it in the UI, e.g. "winget".
        manager: &'static str,
        program: &'static str,
        args: &'static [&'static str],
    },
    /// Commands for the user to run themselves. The app never executes these.
    Manual { options: &'static [&'static str] },
}

impl InstallRoute {
    /// The command line as one string, for display.
    pub fn display_command(&self) -> String {
        match self {
            InstallRoute::Managed { program, args, .. } => std::iter::once(*program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
            InstallRoute::Manual { options } => options.join("\n"),
        }
    }
}

/// The install route for the platform this build is running on.
pub fn install_route() -> InstallRoute {
    #[cfg(target_os = "windows")]
    {
        // winget ships with Windows 10 1809+ and Windows 11. `-e` pins the exact
        // package id so a partial-name match cannot install something else, and
        // the two `--accept-*` flags keep it from stopping on an interactive
        // prompt we have no console to answer.
        InstallRoute::Managed {
            manager: "winget",
            program: "winget",
            args: &[
                "install",
                "--id",
                "Gyan.FFmpeg",
                "-e",
                "--source",
                "winget",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ],
        }
    }
    #[cfg(target_os = "macos")]
    {
        InstallRoute::Manual {
            options: &["brew install ffmpeg"],
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        InstallRoute::Manual {
            options: &[
                "sudo apt install ffmpeg",
                "sudo dnf install ffmpeg",
                "flatpak install flathub org.freedesktop.Platform.ffmpeg-full",
            ],
        }
    }
}

/// Build a `Command` that does not flash a console window on Windows.
fn quiet_command(program: &Path) -> Command {
    let command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW. ffmpeg is a console program, so without this every
        // export and every probe pops a black window in the user's face.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        return command;
    }
    #[cfg(not(target_os = "windows"))]
    command
}

/// Whether `candidate` is an ffmpeg that runs.
fn responds_to_version(candidate: &Path) -> bool {
    quiet_command(candidate)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Extra places to look beyond `PATH`.
///
/// After `winget install` the package's shim lands in a directory that is on the
/// *user's* PATH, but this process inherited its environment at launch and will
/// not see it until it restarts. Probing the shim directory directly is what
/// lets an install take effect without the user having to relaunch Manul.
fn extra_search_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|local| {
                vec![
                    PathBuf::from(local)
                        .join("Microsoft")
                        .join("WinGet")
                        .join("Links"),
                ]
            })
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        // Homebrew's two prefixes, for the same reason: a GUI app launched from
        // Finder does not inherit a login shell's PATH.
        vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Vec::new()
    }
}

const EXECUTABLE: &str = if cfg!(target_os = "windows") {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};

/// Find a working ffmpeg, or `None`.
///
/// Tries the bare name first so a `PATH` entry wins, then the platform's
/// well-known install directories.
pub fn locate() -> Option<PathBuf> {
    let on_path = PathBuf::from(EXECUTABLE);
    if responds_to_version(&on_path) {
        return Some(on_path);
    }
    extra_search_dirs()
        .into_iter()
        .map(|dir| dir.join(EXECUTABLE))
        .find(|candidate| candidate.is_file() && responds_to_version(candidate))
}

/// Run the platform's package manager to install ffmpeg.
///
/// Only ever called after the user has agreed to the exact command in
/// [`InstallRoute::display_command`]. Blocks until the manager exits, so the
/// caller should be on a worker thread.
pub fn run_install(route: &InstallRoute) -> Result<(), String> {
    let InstallRoute::Managed { program, args, .. } = route else {
        return Err("this platform installs ffmpeg through its own package manager".to_string());
    };

    let output = quiet_command(Path::new(program))
        .args(*args)
        .output()
        .map_err(|err| format!("could not run {program}: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    // winget writes its diagnostics to stdout, not stderr.
    let detail = [&output.stdout[..], &output.stderr[..]]
        .concat()
        .iter()
        .map(|&b| b as char)
        .collect::<String>();
    let detail = detail.trim();
    let tail: String = detail.lines().rev().take(4).collect::<Vec<_>>().join(" / ");
    Err(if tail.is_empty() {
        format!("{program} exited with {}", output.status)
    } else {
        format!("{program} failed: {tail}")
    })
}

/// Quality of the encoded video, as an x264 constant-rate factor.
///
/// Lower is better and bigger. The three offered here span "good enough for a
/// slide" to "archival"; the numbers are x264's usual sane range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Small,
    Balanced,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Small, Quality::Balanced, Quality::High];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Small => "Small file",
            Quality::Balanced => "Balanced",
            Quality::High => "High quality",
        }
    }

    fn crf(self) -> &'static str {
        match self {
            Quality::Small => "28",
            Quality::Balanced => "20",
            Quality::High => "14",
        }
    }
}

/// Encode the PNG sequence in `frames_dir` into `output`.
///
/// Blocks until ffmpeg exits; call it from a worker thread.
///
/// `yuv420p` is not the encoder's preferred format for this input, but it is the
/// one every player and every slideshow tool can actually decode — without it
/// the file is technically valid H.264 that QuickTime and PowerPoint refuse to
/// open. It requires even dimensions, which the caller guarantees.
pub fn encode(
    ffmpeg: &Path,
    frames_dir: &Path,
    fps: u32,
    quality: Quality,
    output: &Path,
) -> Result<(), String> {
    let output_status = quiet_command(ffmpeg)
        .arg("-y")
        .arg("-framerate")
        .arg(fps.max(1).to_string())
        .arg("-i")
        .arg(frames_dir.join(FRAME_PATTERN))
        .args(["-c:v", "libx264"])
        .args(["-pix_fmt", "yuv420p"])
        .args(["-crf", quality.crf()])
        .args(["-preset", "medium"])
        // Without this a player that seeks by keyframe can only land every 250
        // frames, which on a 10 s clip means no seeking at all.
        .args(["-g", "30"])
        .arg(output)
        .output()
        .map_err(|err| format!("could not run ffmpeg: {err}"))?;

    if output_status.status.success() {
        return Ok(());
    }

    let stderr: String = output_status.stderr.iter().map(|&b| b as char).collect();
    let tail: String = stderr
        .trim()
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    Err(if tail.is_empty() {
        format!("ffmpeg exited with {}", output_status.status)
    } else {
        format!("ffmpeg failed: {tail}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_names_sort_in_playback_order() {
        // ffmpeg's %06d pattern matches these positionally, so a name that sorts
        // out of order would splice the video out of order.
        let mut names: Vec<String> = [0usize, 9, 10, 99, 100, 1000]
            .iter()
            .map(|&i| frame_file_name(i))
            .collect();
        let sorted = {
            let mut copy = names.clone();
            copy.sort();
            copy
        };
        assert_eq!(names, sorted);
        names.dedup();
        assert_eq!(names.len(), 6, "names must be unique");
    }

    #[test]
    fn frame_file_names_are_recognised_but_neighbours_are_not() {
        for index in [0usize, 7, 1234, 999_999, 1_000_000] {
            let name = frame_file_name(index);
            assert!(is_frame_file_name(&name), "{name} should be ours");
        }
        for other in [
            "notes.png",
            "frame_1.png",
            "frame_00001.png",
            "frame_000001.jpg",
            "frame_abcdef.png",
            "frame_.png",
            "prefix_frame_000001.png",
            "figure.pdf",
        ] {
            assert!(!is_frame_file_name(other), "{other} is not ours to delete");
        }
    }

    #[test]
    fn frame_names_match_the_pattern_width() {
        assert_eq!(frame_file_name(0), "frame_000000.png");
        assert_eq!(frame_file_name(123_456), "frame_123456.png");
        assert!(FRAME_PATTERN.contains("%06d"));
    }

    #[test]
    fn every_platform_offers_a_route_with_a_command_to_show() {
        let route = install_route();
        assert!(
            !route.display_command().trim().is_empty(),
            "the consent dialog has nothing to show the user"
        );
    }

    #[test]
    fn manual_routes_are_never_run_by_run_install() {
        let manual = InstallRoute::Manual {
            options: &["sudo apt install ffmpeg"],
        };
        assert!(run_install(&manual).is_err());
    }

    #[test]
    fn quality_levels_are_ordered_from_smallest_to_best() {
        let crfs: Vec<u32> = Quality::ALL
            .iter()
            .map(|q| q.crf().parse().unwrap())
            .collect();
        assert!(
            crfs.windows(2).all(|w| w[0] > w[1]),
            "a lower CRF is higher quality, so these must descend: {crfs:?}"
        );
    }
}
