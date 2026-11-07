use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub const SAND_WALLPAPER_COMMAND: &str = "/usr/local/bin/sand-wallpaper";
pub const SAND_WALLPAPER_TONE_SCRIPT: &str = "/usr/local/bin/sand-wallpaper-tone.mjs";
pub const X_SOCKET_DIR: &str = "/tmp/.X11-unix";
pub const COMMAND_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperPlan {
    pub tone: String,
    pub ms_until_next_boundary: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayOwner {
    pub display: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandRunOptions {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

pub type WallpaperCommandRunner = Arc<
    dyn Fn(&Path, &[String], CommandRunOptions) -> Result<String, String> + Send + Sync + 'static,
>;
pub type WallpaperDisplayReader =
    Arc<dyn Fn(&Path) -> Result<Vec<DisplayOwner>, String> + Send + Sync + 'static>;
pub type WallpaperExists = Arc<dyn Fn(&Path) -> bool + Send + Sync + 'static>;
pub type WallpaperLog = Arc<dyn Fn(&str) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct BoxWallpaperCommandOptions {
    pub settings_path: PathBuf,
    pub log: WallpaperLog,
    pub node_path: PathBuf,
    pub tone_script_path: PathBuf,
    pub wallpaper_command_path: PathBuf,
    pub x_socket_dir: PathBuf,
    pub run_command: WallpaperCommandRunner,
    pub read_displays: WallpaperDisplayReader,
    pub exists: WallpaperExists,
}

impl BoxWallpaperCommandOptions {
    pub fn production(settings_path: PathBuf, log: WallpaperLog) -> Self {
        let node_path = std::env::var_os("SAND_NODE_BINARY")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("node"));
        Self {
            settings_path,
            log,
            node_path,
            tone_script_path: PathBuf::from(SAND_WALLPAPER_TONE_SCRIPT),
            wallpaper_command_path: PathBuf::from(SAND_WALLPAPER_COMMAND),
            x_socket_dir: PathBuf::from(X_SOCKET_DIR),
            run_command: Arc::new(default_run_command),
            read_displays: Arc::new(|path| read_live_displays(path).map_err(|error| error.to_string())),
            exists: Arc::new(|path| path.exists()),
        }
    }
}

#[derive(Clone)]
pub struct BoxWallpaperCommands {
    options: BoxWallpaperCommandOptions,
}

impl BoxWallpaperCommands {
    pub fn new(options: BoxWallpaperCommandOptions) -> Self {
        Self { options }
    }

    pub fn production(settings_path: PathBuf, log: WallpaperLog) -> Self {
        Self::new(BoxWallpaperCommandOptions::production(settings_path, log))
    }

    pub fn is_available(&self) -> bool {
        (self.options.exists)(&self.options.tone_script_path)
            && (self.options.exists)(&self.options.wallpaper_command_path)
    }

    pub fn resolve_plan(&self) -> Option<WallpaperPlan> {
        let args = vec![
            self.options.tone_script_path.to_string_lossy().into_owned(),
            self.options.settings_path.to_string_lossy().into_owned(),
        ];
        let result = match (self.options.run_command)(
            &self.options.node_path,
            &args,
            CommandRunOptions::default(),
        ) {
            Ok(stdout) => stdout,
            Err(error) => {
                (self.options.log)(&format!("wallpaper: tone helper failed ({error})"));
                return None;
            }
        };
        let plan = parse_tone_plan(&result);
        if plan.is_none() {
            (self.options.log)("wallpaper: tone helper printed an unusable plan");
        }
        plan
    }

    pub fn paint(&self) {
        let displays = match (self.options.read_displays)(&self.options.x_socket_dir) {
            Ok(displays) => displays,
            Err(error) => {
                (self.options.log)(&format!("wallpaper: reading live displays failed ({error})"));
                return;
            }
        };
        for owner in displays {
            let args = vec!["paint".to_string(), owner.display.clone()];
            let command_options = as_display_owner(
                &owner,
                current_user_id(),
                current_group_id(),
            );
            if let Err(error) = (self.options.run_command)(
                &self.options.wallpaper_command_path,
                &args,
                command_options,
            ) {
                (self.options.log)(&format!(
                    "wallpaper: paint of {} failed ({error})",
                    owner.display
                ));
            }
        }
    }
}

pub fn parse_tone_plan(stdout: &str) -> Option<WallpaperPlan> {
    let parts = stdout.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }
    let tone = parts[0];
    let seconds = parts[1];
    if tone.is_empty()
        || !tone.bytes().all(|byte| byte.is_ascii_lowercase())
        || seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let seconds = seconds.parse::<u64>().ok()?;
    Some(WallpaperPlan {
        tone: tone.to_string(),
        ms_until_next_boundary: seconds.saturating_mul(1_000),
    })
}

pub fn as_display_owner(
    owner: &DisplayOwner,
    self_uid: Option<u32>,
    self_gid: Option<u32>,
) -> CommandRunOptions {
    let Some(self_uid) = self_uid else {
        return CommandRunOptions::default();
    };
    if self_uid != 0 || (owner.uid == self_uid && Some(owner.gid) == self_gid) {
        return CommandRunOptions::default();
    }
    CommandRunOptions {
        uid: Some(owner.uid),
        gid: Some(owner.gid),
    }
}

#[cfg(unix)]
fn current_user_id() -> Option<u32> {
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn current_user_id() -> Option<u32> {
    None
}

#[cfg(unix)]
fn current_group_id() -> Option<u32> {
    Some(unsafe { libc::getegid() })
}

#[cfg(not(unix))]
fn current_group_id() -> Option<u32> {
    None
}

#[cfg(unix)]
pub fn read_live_displays(socket_dir: &Path) -> std::io::Result<Vec<DisplayOwner>> {
    let entries = match fs::read_dir(socket_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    let mut candidates = Vec::<(u32, PathBuf)>::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix('X') else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(number) = number.parse::<u32>() else {
            continue;
        };
        candidates.push((number, entry.path()));
    }
    candidates.sort_by_key(|(number, _)| *number);

    let mut displays = Vec::new();
    for (number, path) in candidates {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_socket() {
            continue;
        }
        displays.push(DisplayOwner {
            display: format!(":{number}"),
            uid: metadata.uid(),
            gid: metadata.gid(),
        });
    }
    Ok(displays)
}

#[cfg(not(unix))]
pub fn read_live_displays(_socket_dir: &Path) -> std::io::Result<Vec<DisplayOwner>> {
    Ok(Vec::new())
}

fn default_run_command(
    file: &Path,
    args: &[String],
    options: CommandRunOptions,
) -> Result<String, String> {
    let mut command = Command::new(file);
    command.args(args).stdout(Stdio::piped()).stderr(Stdio::null());

    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        if let Some(gid) = options.gid {
            command.gid(gid);
        }
        if let Some(uid) = options.uid {
            command.uid(uid);
        }
    }

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(COMMAND_TIMEOUT_MS);
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => {
                let mut stdout = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    pipe.read_to_string(&mut stdout)
                        .map_err(|error| error.to_string())?;
                }
                return if status.success() {
                    Ok(stdout)
                } else {
                    Err(format!("command exited with {status}"))
                };
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("command timed out after {COMMAND_TIMEOUT_MS}ms"));
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
}
