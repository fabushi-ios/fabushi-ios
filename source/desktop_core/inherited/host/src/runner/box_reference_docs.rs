use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::host_paths::{
    SAND_BOX_DATA_ROOT, SAND_BOX_MODEL_VISIBLE_DATA_ROOT, ensure_data_root_alias,
    get_sand_root_dir,
};

pub const SAND_BOX_REFERENCE_DIR: &str = "/home/box/reference";
pub const LEGACY_SAND_BOX_REFERENCE_DIR: &str = "/home/box/sand-reference";
pub const DEBUGGING_THE_BOX_FILE: &str = "debugging-the-box.md";
pub const SAND_APP_UI_FILE: &str = "app-ui.md";
pub const SAND_BOX_DEBUGGING_REFERENCE_PATH: &str = "/home/box/reference/debugging-the-box.md";
pub const SAND_APP_UI_REFERENCE_PATH: &str = "/home/box/reference/app-ui.md";

pub const SAND_BOX_DEBUGGING_REFERENCE_DOC: &str = r####"# Debugging the box

When the box acts up (won't start, Shell or Screenshot calls fail, a computerUse subagent reports Computer failures, or the desktop won't render), diagnose it yourself before giving up, and keep the user posted with a plain status instead of going silent.
- Is it up? If a Shell command returns output, the box is running and its daemon is healthy. If a box tool instead comes back saying the computer is still starting up (its image is downloading or it's booting), that's transient: wait a few seconds and retry, since a first boot or image pull can take minutes. If Shell and Screenshot aren't offered to you at all, the box substrate is down; in the local Docker setup that means Docker isn't running, which the user fixes from the app's "computer needs Docker" prompt.
- Run the self-check. The box ships a box-doctor health check that runs once at startup and on demand: run `box-doctor` over Shell to probe the live box, or read its last startup result at /tmp/box-doctor.log (its summary also lands in the box's startup log alongside the other /tmp logs). It verifies the handful of things that silently break the box (a valid /etc/machine-id, Chrome and its version, DNS/egress, the system clock, and the D-Bus session bus) and prints one `[box-doctor] PASS|FAIL <name>: <detail>` line per check plus a final `[box-doctor] SUMMARY`. When a page or login times out for no clear reason, run this first and report the failing check to the user instead of guessing.
- Desktop not rendering? Capture it with Screenshot to see the real screen, then use Shell only for read-only diagnostics. The primary desktop is display :1, so xdpyinfo -display :1 confirms the X server is up. The desktop comes up with no browser window, so no Chrome process is normal until a computerUse subagent opens it. Each desktop piece logs under /tmp on the box (start-desktop.log for the overall bringup, plus x11vnc:1.log and novnc:1.log), so tail those to see which one failed; a stale X or Chrome lock left over from a wake is a known cause. If Chrome itself will not start, launch it from Shell with the box's own `box-chrome` launcher (never a raw chrome binary), then inspect the resulting process and logs with Shell; don't drive GUI apps from Shell with input automation such as xdotool or Shell CDP.
- Which runtime, and is it healthy? The box runs either as a local Docker container (dev) or a brokered anyrun pod (the shipped default), behind the same Shell and Screenshot surfaces plus the Computer tool delegated to computerUse subagents. Tell them apart by testing for /.dockerenv from Shell (present means Docker, absent means anyrun). On Docker you can inspect the runtime straight from ExternalShell on the user's computer with docker ps, docker logs, and docker inspect on the sand-box- container, and a stopped Docker daemon is why the box won't come up. On anyrun the pod's lifecycle is managed server-side, so there's nothing to inspect locally; lean on the in-box probes above.
- Commands failing? Check the basics over Shell: df -h /workspace for disk (your persistent scratch space) plus the command's own error text. Files and installed tools persist across turns, so a tool that went missing just needs reinstalling.
- Next steps: retry first, since most failures are just a box still booting. You can't rebuild the box yourself, so if it's wedged or stuck on a stale image, surface a clear status and tell the user to recover it from Settings → Updates tab → "Update Fabushi's Computer" (its button says "Update") — it moves the box to a fresh instance while keeping files and logins, and can unstick a wedged box without data loss. That is the recovery action to point users at; the "Reset Fabushi's Computer" row below it restores from the last saved snapshot and can lose recent unsynced work, so never direct the user to it. request_box_help is for handing the user a manual step on a working desktop (a login or captcha), not a repair tool.
"####;
pub const SAND_APP_UI_REFERENCE_DOC: &str = r####"# The Fabushi app UI (real paths — never invent others)

A compact map of Fabushi's real interface so you can guide the user or self-recover. Use only what's listed here; for anything else, follow "Never fabricate data" and say you're unsure rather than inventing a path.
- Opening settings: the sidebar account button at the bottom-left (avatar + account name), the Cmd+, shortcut, or the command palette's "Open settings". There's no gear icon or macOS Preferences menu item.
- Deleting an agent: the user does this from the sidebar — right-click the agent's row and choose "Delete" (a permanent delete that removes the agent and its transcript, with a confirm). It's not in Settings; there's no archive or hide, just this permanent delete.
- Settings has five tabs: General, Plugins, Team Setup, Appearance, Updates.
- General: the account card ("Sign In with Cursor" / "Sign Out").
- Plugins: tools and skills for Fabushi, with a "Search plugins" field and two views. "Marketplace" lists plugins to browse or search; opening one shows its detail page with Add (or Uninstall once installed) and an Accounts card with per-connector Authenticate. "Yours" lists "Installed" plugins (each row shows the live connector status, with a one-click Authenticate when sign-in is needed) and "Private" skills (a per-agent enable toggle; opening one edits its name, description, and instructions, or deletes it).
- Team Setup: scripts installed on every computer assigned to the current team.
- Appearance: "Theme" (System / Light / Dark).
- Updates: box recovery is "Update Fabushi's Computer" (its button says "Update"; data-preserving — it moves the box to a fresh instance while keeping files and logins), a two-click confirm ("Click Again to Confirm"). The "Reset Fabushi's Computer" row (button "Reset") is the destructive recovery of last resort: it restores from the last saved snapshot and can lose recent unsynced work, so steer users to Update instead. Updates also has "Update Track" (Stable / Nightly) and "Check for Updates", which update the Fabushi app itself, distinct from "Update Fabushi's Computer" (which recreates the box).
- Per-agent info pane (separate from the global Settings): open it by clicking the agent's name in the chat header (or Cmd+Shift+I), close it with the "X" in the pane's own header. It shows a live preview of that agent's computer (click it to open the full screen view) over its Routines list, plus Channels when a channel connector is available to connect or one is already connected, and Members in group chats. The gear beside the "X" opens a per-agent Settings subpage (avatar, name, title, description, and per-assistant notifications).
"####;

pub const SAND_BOX_REFERENCE_DOCS: &[(&str, &str)] = &[
    (DEBUGGING_THE_BOX_FILE, SAND_BOX_DEBUGGING_REFERENCE_DOC),
    (SAND_APP_UI_FILE, SAND_APP_UI_REFERENCE_DOC),
];

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!(
        "{}.{}.{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
        process::id(),
        nonce,
    ));
    let mut file = File::create(&temp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

pub fn write_sand_box_reference_docs(
    reference_dir: impl AsRef<Path>,
) -> io::Result<Vec<PathBuf>> {
    let reference_dir = reference_dir.as_ref();
    let mut written = Vec::with_capacity(SAND_BOX_REFERENCE_DOCS.len());
    for (file_name, contents) in SAND_BOX_REFERENCE_DOCS {
        let path = reference_dir.join(file_name);
        atomic_write(&path, contents.as_bytes())?;
        written.push(path);
    }
    Ok(written)
}

pub fn provision_sand_box_prompt_artifacts_with(
    sand_root_dir: &Path,
    reference_dir: &Path,
    legacy_reference_dir: &Path,
    data_root: &Path,
    model_visible_data_root: &Path,
) -> io::Result<Vec<PathBuf>> {
    if sand_root_dir == data_root {
        ensure_data_root_alias(data_root, model_visible_data_root)?;
    }

    let written = write_sand_box_reference_docs(reference_dir)?;
    match fs::remove_dir_all(legacy_reference_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(written)
}

pub fn provision_sand_box_prompt_artifacts() -> io::Result<Vec<PathBuf>> {
    provision_sand_box_prompt_artifacts_with(
        &get_sand_root_dir(),
        Path::new(SAND_BOX_REFERENCE_DIR),
        Path::new(LEGACY_SAND_BOX_REFERENCE_DIR),
        Path::new(SAND_BOX_DATA_ROOT),
        Path::new(SAND_BOX_MODEL_VISIBLE_DATA_ROOT),
    )
}
