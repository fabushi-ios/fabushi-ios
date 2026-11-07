use std::fs;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::attachments::video_playback_rendition::{
    FAILED_RESOLUTION_RETRY_MS, VideoPlaybackResolver, VideoToolRunner,
};

fn temp_dir(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-video-playback-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn non_hevc_video_uses_original_source_without_transcode() {
    let root = temp_dir("passthrough");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("clip.mp4");
    fs::write(&source, b"source").unwrap();

    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = Arc::clone(&calls);
    let runner: VideoToolRunner = Arc::new(move |command, _args| {
        observed.lock().unwrap().push(command.to_string());
        if command == "ffprobe" {
            Ok("h264\n".into())
        } else {
            Err("unexpected transcode".into())
        }
    });
    let resolver = VideoPlaybackResolver::new(
        runner,
        Arc::new(|| 1_000),
        root.join("cache"),
    );

    let resolved = resolver
        .with_source(&source, |path| Ok::<_, ()>(path.to_path_buf()))
        .unwrap();
    assert_eq!(resolved, source);
    assert_eq!(*calls.lock().unwrap(), vec!["ffprobe"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn hevc_video_transcodes_once_and_reuses_cached_rendition() {
    let root = temp_dir("hevc");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("clip.mov");
    fs::write(&source, b"hevc-source").unwrap();

    let ffmpeg_calls = Arc::new(AtomicUsize::new(0));
    let ffmpeg_for_runner = Arc::clone(&ffmpeg_calls);
    let runner: VideoToolRunner = Arc::new(move |command, args| {
        match command {
            "ffprobe" => Ok("hevc\n".into()),
            "ffmpeg" => {
                ffmpeg_for_runner.fetch_add(1, Ordering::AcqRel);
                let output = PathBuf::from(args.last().expect("ffmpeg output path"));
                fs::write(output, b"h264-rendition").map_err(|error| error.to_string())?;
                Ok(String::new())
            }
            other => Err(format!("unexpected command {other}")),
        }
    });
    let resolver = VideoPlaybackResolver::new(
        runner,
        Arc::new(|| 5_000),
        root.join("cache"),
    );

    let first = resolver
        .with_source(&source, |path| {
            Ok::<_, ()>((path.to_path_buf(), fs::read(path).unwrap()))
        })
        .unwrap();
    assert_ne!(first.0, source);
    assert_eq!(first.1, b"h264-rendition");

    let second = resolver
        .with_source(&source, |path| Ok::<_, ()>(path.to_path_buf()))
        .unwrap();
    assert_eq!(second, first.0);
    assert_eq!(ffmpeg_calls.load(Ordering::Acquire), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_hevc_resolution_falls_back_then_retries_after_frozen_backoff() {
    let root = temp_dir("retry");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("clip.mp4");
    fs::write(&source, b"hevc-source").unwrap();

    let now = Arc::new(AtomicU64::new(10_000));
    let now_for_clock = Arc::clone(&now);
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_runner = Arc::clone(&attempts);
    let runner: VideoToolRunner = Arc::new(move |command, args| match command {
        "ffprobe" => Ok("hevc".into()),
        "ffmpeg" => {
            let attempt = attempts_for_runner.fetch_add(1, Ordering::AcqRel);
            if attempt == 0 {
                return Err("transcode failed".into());
            }
            fs::write(
                PathBuf::from(args.last().expect("ffmpeg output path")),
                b"retry-rendition",
            )
            .map_err(|error| error.to_string())?;
            Ok(String::new())
        }
        other => Err(format!("unexpected command {other}")),
    });
    let resolver = VideoPlaybackResolver::new(
        runner,
        Arc::new(move || now_for_clock.load(Ordering::Acquire)),
        root.join("cache"),
    );

    let first = resolver
        .with_source(&source, |path| Ok::<_, ()>(path.to_path_buf()))
        .unwrap();
    assert_eq!(first, source);
    let before_retry = resolver
        .with_source(&source, |path| Ok::<_, ()>(path.to_path_buf()))
        .unwrap();
    assert_eq!(before_retry, source);
    assert_eq!(attempts.load(Ordering::Acquire), 1);

    now.store(
        10_000 + FAILED_RESOLUTION_RETRY_MS + 1,
        Ordering::Release,
    );
    let after_retry = resolver
        .with_source(&source, |path| Ok::<_, ()>(path.to_path_buf()))
        .unwrap();
    assert_ne!(after_retry, source);
    assert_eq!(fs::read(after_retry).unwrap(), b"retry-rendition");
    assert_eq!(attempts.load(Ordering::Acquire), 2);
    let _ = fs::remove_dir_all(root);
}
