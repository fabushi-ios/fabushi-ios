//! Thin WebAssembly C-ABI adapter over `fabushi_miniapp`.
//!
//! All business logic, state transitions, sutras, country coordinates, achievements,
//! 2D/3D globe spherical projection, and Zen Buddha/incense/smoke geometry live in Rust.

use fabushi_miniapp::scene_math::{
    normalize_longitude, shortest_longitude_delta, CanvasDrawCmd,
};
use fabushi_miniapp::state::{
    FabushiClientState, HttpActionEffect, HttpResponsePayload, UiCommand, ViewModelSnapshot,
};
use serde::Serialize;
use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;

const GLOBE_COLS: usize = 48;
const GLOBE_ROWS: usize = 36;

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WasmEffect {
    Http {
        method: &'static str,
        url: String,
        body: Option<serde_json::Value>,
        response_type: &'static str,
    },
    CloudStorageSet {
        key: String,
        value: String,
    },
    SendData {
        data: String,
    },
}

#[derive(Serialize)]
struct WasmDispatchEnvelope {
    ui_revision: u64,
    snapshot: ViewModelSnapshot,
    draw_commands: Vec<serde_json::Value>,
    effects: Vec<WasmEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct WasmRuntime {
    state: FabushiClientState,
    last_now_ms: f64,
    ui_revision: u64,
    last_result_len: usize,
    globe_slices: Vec<f32>,
    last_transferring: bool,
    last_country: String,
    last_toast: Option<String>,
}

impl WasmRuntime {
    fn new() -> Self {
        Self {
            state: FabushiClientState::new(0.0),
            last_now_ms: 0.0,
            ui_revision: 1,
            last_result_len: 0,
            globe_slices: Vec::with_capacity(GLOBE_COLS * GLOBE_ROWS * 8),
            last_transferring: false,
            last_country: String::new(),
            last_toast: None,
        }
    }
}

thread_local! {
    static RUNTIME: RefCell<WasmRuntime> = RefCell::new(WasmRuntime::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn wasm_alloc(size: usize) -> *mut u8 {
    let layout = match Layout::from_size_align(size.max(1), 1) {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    unsafe { alloc(layout) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wasm_dealloc(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(size.max(1), 1) {
        unsafe { dealloc(ptr, layout) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_result_len() -> usize {
    RUNTIME.with(|rt| rt.borrow().last_result_len)
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_globe_slices_ptr() -> *const f32 {
    RUNTIME.with(|rt| rt.borrow().globe_slices.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_globe_slices_len() -> usize {
    RUNTIME.with(|rt| rt.borrow().globe_slices.len())
}

fn read_utf8<'a>(ptr: *const u8, len: usize) -> Result<&'a str, String> {
    if ptr.is_null() || len == 0 {
        return Err("empty input buffer".to_string());
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(slice).map_err(|e| format!("invalid utf-8: {e}"))
}

fn alloc_result_string(rt: &mut WasmRuntime, json_str: &str) -> *mut u8 {
    let bytes = json_str.as_bytes();
    let ptr = wasm_alloc(bytes.len());
    if ptr.is_null() {
        rt.last_result_len = 0;
        return std::ptr::null_mut();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    }
    rt.last_result_len = bytes.len();
    ptr
}

fn sanitize_pos(v: f64, fallback: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        fallback
    }
}

fn sanitize_nonneg(v: f64, fallback: f64) -> f64 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}

fn inv_project_disc(
    nx: f64,
    ny: f64,
    rot_lon_deg: f64,
    cos_t: f64,
    sin_t: f64,
) -> (f64, f64) {
    let r2 = (nx * nx + ny * ny).min(0.9999);
    let z1 = (1.0 - r2).max(0.0).sqrt();
    let y0 = ny * cos_t + z1 * sin_t;
    let z0 = -ny * sin_t + z1 * cos_t;
    let lat = y0.clamp(-1.0, 1.0).asin().to_degrees();
    let rel_lng = nx.atan2(z0).to_degrees();
    let lon = normalize_longitude(rot_lon_deg + rel_lng);
    (lat, lon)
}

/// Computes equirectangular texture slice rects (`[sx, sy, sw, sh, dx, dy, dw, dh]`,
/// with `sx/sy/sw/sh` normalized in `[0, 1]`) in pure Rust so JS performs zero trig.
fn compute_globe_slices(
    cx: f64,
    cy: f64,
    radius: f64,
    rot_lon_deg: f64,
    tilt_lat_deg: f64,
    out: &mut Vec<f32>,
) -> usize {
    out.clear();
    if radius <= 1.0 || !radius.is_finite() {
        return 0;
    }

    let tilt = tilt_lat_deg.to_radians();
    let cos_t = tilt.cos();
    let sin_t = tilt.sin();
    let cell_w = (2.0 * radius) / GLOBE_COLS as f64;
    let cell_h = (2.0 * radius) / GLOBE_ROWS as f64;
    let mut count = 0usize;

    for col in 0..GLOBE_COLS {
        let x0 = -radius + col as f64 * cell_w;
        let x1 = x0 + cell_w + 0.85;
        let nx0 = (x0 / radius).clamp(-0.995, 0.995);
        let nx1 = ((x0 + cell_w) / radius).clamp(-0.995, 0.995);
        let mid_x = ((nx0 + nx1) * 0.5).clamp(-0.995, 0.995);

        for row in 0..GLOBE_ROWS {
            let y0 = -radius + row as f64 * cell_h;
            let y1 = y0 + cell_h + 0.85;
            let ny0 = -(y0 / radius).clamp(-0.995, 0.995);
            let ny1 = -((y0 + cell_h) / radius).clamp(-0.995, 0.995);
            let mid_y = ((ny0 + ny1) * 0.5).clamp(-0.995, 0.995);

            if mid_x * mid_x + mid_y * mid_y > 1.02 {
                continue;
            }

            let (lat_c, lon_c) = inv_project_disc(mid_x, mid_y, rot_lon_deg, cos_t, sin_t);
            let (_, lon_l) = inv_project_disc(nx0, mid_y, rot_lon_deg, cos_t, sin_t);
            let (_, lon_r) = inv_project_disc(nx1, mid_y, rot_lon_deg, cos_t, sin_t);
            let (lat_t, _) = inv_project_disc(mid_x, ny0, rot_lon_deg, cos_t, sin_t);
            let (lat_b, _) = inv_project_disc(mid_x, ny1, rot_lon_deg, cos_t, sin_t);

            let sw = ((shortest_longitude_delta(lon_l, lon_r).abs() / 360.0) * 1.25)
                .clamp(1.0 / 512.0, 0.25);
            let sh = (((lat_t - lat_b).abs() / 180.0) * 1.18).clamp(1.0 / 256.0, 0.25);

            let u_center = (lon_c + 180.0) / 360.0;
            let v_center = (90.0 - lat_c) / 180.0;
            let sx = (u_center - sw * 0.5).rem_euclid(1.0);
            let sy = (v_center - sh * 0.5).clamp(0.0, (1.0 - sh).max(0.0));
            let dx = cx + x0;
            let dy = cy + y0;
            let dw = x1 - x0;
            let dh = y1 - y0;

            if sx + sw <= 1.0 {
                out.extend_from_slice(&[
                    sx as f32, sy as f32, sw as f32, sh as f32, dx as f32, dy as f32, dw as f32,
                    dh as f32,
                ]);
                count += 1;
            } else {
                let first_w = (1.0 - sx).max(1.0 / 1024.0);
                let first_ratio = (first_w / sw).clamp(0.0, 1.0);
                let second_w = (sw - first_w).max(1.0 / 1024.0);
                out.extend_from_slice(&[
                    sx as f32,
                    sy as f32,
                    first_w as f32,
                    sh as f32,
                    dx as f32,
                    dy as f32,
                    (dw * first_ratio) as f32,
                    dh as f32,
                ]);
                out.extend_from_slice(&[
                    0.0_f32,
                    sy as f32,
                    second_w as f32,
                    sh as f32,
                    (dx + dw * first_ratio) as f32,
                    dy as f32,
                    (dw * (1.0 - first_ratio)) as f32,
                    dh as f32,
                ]);
                count += 2;
            }
        }
    }

    count
}

fn translate_effect(eff: HttpActionEffect) -> WasmEffect {
    match eff {
        HttpActionEffect::Bootstrap => WasmEffect::Http {
            method: "GET",
            url: "/api/v1/fabushi-miniapp/bootstrap".to_string(),
            body: None,
            response_type: "bootstrap",
        },
        HttpActionEffect::RecordTransfer {
            content_title,
            content_kind,
            target_countries,
            bytes_sent,
            beam_count,
        } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/transfer".to_string(),
            body: Some(serde_json::json!({
                "content_title": content_title,
                "content_kind": content_kind,
                "target_countries": target_countries,
                "bytes_sent": bytes_sent,
                "beam_count": beam_count,
            })),
            response_type: "transfer_recorded",
        },
        HttpActionEffect::SavePracticeSession {
            sutra,
            duration_minutes,
            chant_count,
            start_time_ms,
            end_time_ms,
            notes,
        } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/practice/session".to_string(),
            body: Some(serde_json::json!({
                "sutra": sutra,
                "duration_minutes": duration_minutes,
                "chant_count": chant_count,
                "start_time_ms": start_time_ms,
                "end_time_ms": end_time_ms,
                "notes": notes,
            })),
            response_type: "practice_session_saved",
        },
        HttpActionEffect::CreatePracticeRecord {
            sutra_name,
            chant_count,
            duration_minutes,
            record_date,
            local_time,
            notes,
        } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/practice/record".to_string(),
            body: Some(serde_json::json!({
                "sutra_name": sutra_name,
                "chant_count": chant_count,
                "duration_minutes": duration_minutes,
                "record_date": record_date,
                "local_time": local_time,
                "notes": notes,
            })),
            response_type: "practice_records_refreshed",
        },
        HttpActionEffect::UpdatePracticeRecord {
            id,
            sutra_name,
            chant_count,
            duration_minutes,
            record_date,
            local_time,
            notes,
        } => WasmEffect::Http {
            method: "PUT",
            url: format!("/api/v1/fabushi-miniapp/practice/record/{id}"),
            body: Some(serde_json::json!({
                "sutra_name": sutra_name,
                "chant_count": chant_count,
                "duration_minutes": duration_minutes,
                "record_date": record_date,
                "local_time": local_time,
                "notes": notes,
            })),
            response_type: "practice_records_refreshed",
        },
        HttpActionEffect::DeletePracticeRecord { id } => WasmEffect::Http {
            method: "DELETE",
            url: format!("/api/v1/fabushi-miniapp/practice/record/{id}"),
            body: None,
            response_type: "practice_records_refreshed",
        },
        HttpActionEffect::UpdateProfile {
            display_name,
            bio,
            locked_practice,
            locale,
        } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/profile".to_string(),
            body: Some(serde_json::json!({
                "display_name": display_name,
                "bio": bio,
                "locked_practice": locked_practice,
                "locale": locale,
            })),
            response_type: "profile_updated",
        },
        HttpActionEffect::ActivateMembership { plan } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/membership".to_string(),
            body: Some(serde_json::json!({ "plan": plan })),
            response_type: "membership_activated",
        },
        HttpActionEffect::ToggleFollow { target_username } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/follow".to_string(),
            body: Some(serde_json::json!({ "target_username": target_username })),
            response_type: "follow_toggled",
        },
        HttpActionEffect::FetchLeaderboard { kind } => {
            let clean_kind = if kind == "practice" { "practice" } else { "global" };
            WasmEffect::Http {
                method: "GET",
                url: format!("/api/v1/fabushi-miniapp/leaderboard?kind={clean_kind}"),
                body: None,
                response_type: "leaderboard_fetched",
            }
        }
        HttpActionEffect::Heartbeat { activity_type } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/heartbeat".to_string(),
            body: Some(serde_json::json!({ "activity_type": activity_type })),
            response_type: "heartbeat_ack",
        },
        HttpActionEffect::SendAiChat { message, history } => WasmEffect::Http {
            method: "POST",
            url: "/api/v1/fabushi-miniapp/ai-chat".to_string(),
            body: Some(serde_json::json!({ "message": message, "history": history })),
            response_type: "ai_chat_reply",
        },
        HttpActionEffect::SyncCloudStorage { key, value } => {
            WasmEffect::CloudStorageSet { key, value }
        }
        HttpActionEffect::EmitWebAppData { data } => WasmEffect::SendData { data },
    }
}

fn serialize_result(
    rt: &mut WasmRuntime,
    res: fabushi_miniapp::state::DispatchResult,
    bump_revision: bool,
    error: Option<String>,
) -> *mut u8 {
    let should_bump = bump_revision
        || !res.effects.is_empty()
        || res.snapshot.is_transferring != rt.last_transferring
        || (res.snapshot.is_transferring
            && res.snapshot.current_sending_country != rt.last_country)
        || res.snapshot.toast_message != rt.last_toast;

    if should_bump {
        rt.ui_revision = rt.ui_revision.wrapping_add(1);
    }
    rt.last_transferring = res.snapshot.is_transferring;
    rt.last_country = res.snapshot.current_sending_country.clone();
    rt.last_toast = res.snapshot.toast_message.clone();

    rt.globe_slices.clear();
    let mut draw_commands = Vec::with_capacity(res.draw_commands.len());
    for cmd in res.draw_commands {
        let mut val = serde_json::to_value(&cmd).unwrap_or(serde_json::Value::Null);
        if let CanvasDrawCmd::DrawGlobeTextureSphere {
            cx,
            cy,
            radius,
            rotation_lon_deg,
            tilt_lat_deg,
            ..
        } = &cmd
        {
            let slice_count = compute_globe_slices(
                *cx,
                *cy,
                *radius,
                *rotation_lon_deg,
                *tilt_lat_deg,
                &mut rt.globe_slices,
            );
            if let Some(obj) = val.as_object_mut() {
                obj.insert("slice_count".to_string(), serde_json::json!(slice_count));
            }
        }
        draw_commands.push(val);
    }

    let effects: Vec<WasmEffect> = res.effects.into_iter().map(translate_effect).collect();
    let envelope = WasmDispatchEnvelope {
        ui_revision: rt.ui_revision,
        snapshot: res.snapshot,
        draw_commands,
        effects,
        error,
    };

    let json = serde_json::to_string(&envelope).unwrap_or_else(|e| {
        format!(r#"{{"ui_revision":0,"error":"serialize error: {e}"}}"#)
    });
    alloc_result_string(rt, &json)
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_init(
    now_ms: f64,
    tz_offset_minutes: i32,
    width: f64,
    height: f64,
) -> *mut u8 {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let safe_now = sanitize_nonneg(now_ms, 0.0);
        let safe_w = sanitize_pos(width, 420.0);
        let safe_h = sanitize_pos(height, 780.0);
        rt.state = FabushiClientState::new(safe_now);
        rt.state.tz_offset_minutes = tz_offset_minutes;
        rt.state.last_viewport_w = safe_w;
        rt.state.last_viewport_h = safe_h;
        rt.last_now_ms = safe_now;
        let res = rt.state.dispatch(UiCommand::InitBootstrap);
        serialize_result(&mut rt, res, true, None)
    })
}

fn current_tick(rt: &mut WasmRuntime) -> fabushi_miniapp::state::DispatchResult {
    let (now, w, h) = (
        rt.last_now_ms,
        rt.state.last_viewport_w,
        rt.state.last_viewport_h,
    );
    rt.state.tick(0.0, now, w, h)
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_set_initial_tab(ptr: *const u8, len: usize) -> *mut u8 {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if let Ok(tab) = read_utf8(ptr, len) {
            rt.state.set_initial_tab_from_query(tab);
        }
        let res = current_tick(&mut rt);
        serialize_result(&mut rt, res, true, None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_tick(
    dt_ms: f64,
    now_ms: f64,
    width: f64,
    height: f64,
) -> *mut u8 {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let safe_now = sanitize_nonneg(now_ms, rt.last_now_ms + 33.0);
        let safe_dt = if dt_ms.is_finite() && dt_ms >= 0.0 {
            dt_ms.min(250.0)
        } else {
            (safe_now - rt.last_now_ms).clamp(0.0, 250.0)
        };
        rt.last_now_ms = safe_now;
        let safe_w = sanitize_pos(width, rt.state.last_viewport_w);
        let safe_h = sanitize_pos(height, rt.state.last_viewport_h);
        let res = rt.state.tick(safe_dt, safe_now, safe_w, safe_h);
        serialize_result(&mut rt, res, false, None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_dispatch(ptr: *const u8, len: usize) -> *mut u8 {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        match read_utf8(ptr, len) {
            Ok(raw) => match serde_json::from_str::<UiCommand>(raw) {
                Ok(cmd) => {
                    let bump = !matches!(cmd, UiCommand::SetComposerText { .. });
                    let res = rt.state.dispatch(cmd);
                    serialize_result(&mut rt, res, bump, None)
                }
                Err(err) => {
                    let res = current_tick(&mut rt);
                    serialize_result(
                        &mut rt,
                        res,
                        false,
                        Some(format!("invalid UiCommand JSON: {err}")),
                    )
                }
            },
            Err(err) => {
                let res = current_tick(&mut rt);
                serialize_result(&mut rt, res, false, Some(err))
            }
        }
    })
}

fn decode_http_payload(raw: &str) -> Result<HttpResponsePayload, String> {
    if let Ok(direct) = serde_json::from_str::<HttpResponsePayload>(raw) {
        return Ok(direct);
    }

    let env: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid HTTP payload JSON: {e}"))?;

    if env.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let msg = env
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| {
                env.get("payload")
                    .and_then(|p| p.get("message").or_else(|| p.get("error")))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("request failed")
            .to_string();
        return Ok(HttpResponsePayload::Error { message: msg });
    }

    let rtype = env
        .get("response_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut payload_val = env.get("payload").cloned().unwrap_or(env);
    if let Some(obj) = payload_val.as_object_mut() {
        if !obj.contains_key("type") && !rtype.is_empty() {
            obj.insert("type".to_string(), serde_json::json!(rtype));
        }
        // Support servers returning raw UserProfileDto directly on membership activation
        if obj.get("type").and_then(|v| v.as_str()) == Some("membership_activated")
            && !obj.contains_key("profile")
            && obj.contains_key("user_id")
        {
            let prof = serde_json::Value::Object(obj.clone());
            obj.clear();
            obj.insert("type".to_string(), serde_json::json!("membership_activated"));
            obj.insert("profile".to_string(), prof);
        }
    }

    serde_json::from_value::<HttpResponsePayload>(payload_val)
        .map_err(|e| format!("incompatible HTTP payload shape: {e}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn fabushi_wasm_apply_http(ptr: *const u8, len: usize) -> *mut u8 {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        match read_utf8(ptr, len) {
            Ok(raw) => match decode_http_payload(raw) {
                Ok(payload) => {
                    let res = rt.state.apply_http_effect(payload);
                    serialize_result(&mut rt, res, true, None)
                }
                Err(err) => {
                    let res = rt.state.apply_http_effect(HttpResponsePayload::Error {
                        message: err.clone(),
                    });
                    serialize_result(&mut rt, res, true, Some(err))
                }
            },
            Err(err) => {
                let res = current_tick(&mut rt);
                serialize_result(&mut rt, res, false, Some(err))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn take_result(ptr: *mut u8) -> String {
        let len = fabushi_wasm_result_len();
        assert!(!ptr.is_null());
        assert!(len > 0);
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        let s = std::str::from_utf8(bytes).expect("valid utf8").to_string();
        wasm_dealloc(ptr, len);
        s
    }

    #[test]
    fn c_abi_init_tick_dispatch_and_apply_http_work_end_to_end() {
        let init_ptr = fabushi_wasm_init(1_000_000.0, 480, 420.0, 780.0);
        let init_json = unsafe { take_result(init_ptr) };
        let init_val: serde_json::Value = serde_json::from_str(&init_json).unwrap();
        assert_eq!(init_val["snapshot"]["active_tab"], "home");
        assert!(fabushi_wasm_globe_slices_len() > 100);
        assert_eq!(
            init_val["effects"][0]["url"],
            "/api/v1/fabushi-miniapp/bootstrap"
        );

        let cmd = r#"{"type":"activate_dharma_mode"}"#;
        let in_ptr = wasm_alloc(cmd.len());
        unsafe { std::ptr::copy_nonoverlapping(cmd.as_ptr(), in_ptr, cmd.len()) };
        let disp_ptr = fabushi_wasm_dispatch(in_ptr, cmd.len());
        wasm_dealloc(in_ptr, cmd.len());
        let disp_json = unsafe { take_result(disp_ptr) };
        let disp_val: serde_json::Value = serde_json::from_str(&disp_json).unwrap();
        assert_eq!(disp_val["snapshot"]["is_dharma_composer_mode"], true);

        let tick_ptr = fabushi_wasm_tick(33.0, 1_000_033.0, 420.0, 780.0);
        let tick_json = unsafe { take_result(tick_ptr) };
        let tick_val: serde_json::Value = serde_json::from_str(&tick_json).unwrap();
        assert!(tick_val["draw_commands"].as_array().unwrap().len() > 10);

        let http_env = r#"{"response_type":"ai_chat_reply","payload":{"reply":"南无阿弥陀佛"}}"#;
        let h_ptr = wasm_alloc(http_env.len());
        unsafe { std::ptr::copy_nonoverlapping(http_env.as_ptr(), h_ptr, http_env.len()) };
        let apply_ptr = fabushi_wasm_apply_http(h_ptr, http_env.len());
        wasm_dealloc(h_ptr, http_env.len());
        let apply_json = unsafe { take_result(apply_ptr) };
        let apply_val: serde_json::Value = serde_json::from_str(&apply_json).unwrap();
        assert_eq!(
            apply_val["snapshot"]["ai_messages"][0]["text"],
            "南无阿弥陀佛"
        );
    }

    #[test]
    fn compute_globe_slices_handles_tilt_and_seam_wrapping() {
        let mut buf = Vec::new();
        let count = compute_globe_slices(210.0, 260.0, 140.0, 179.0, 12.0, &mut buf);
        assert!(count > 400);
        assert_eq!(buf.len(), count * 8);
        assert!(buf.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn tick_does_not_bump_ui_revision_on_progress_timer_or_steady_toast() {
        let init_ptr = fabushi_wasm_init(1_000.0, 480, 420.0, 780.0);
        let _ = unsafe { take_result(init_ptr) };

        let dispatch_cmd = |cmd: &str| -> serde_json::Value {
            let p = wasm_alloc(cmd.len());
            unsafe { std::ptr::copy_nonoverlapping(cmd.as_ptr(), p, cmd.len()) };
            let out = fabushi_wasm_dispatch(p, cmd.len());
            wasm_dealloc(p, cmd.len());
            serde_json::from_str(&unsafe { take_result(out) }).unwrap()
        };

        // Start a transfer so progress pct ticks continuously; ui_revision must remain flat until hop/country changes
        dispatch_cmd(r#"{"type":"activate_dharma_mode"}"#);
        dispatch_cmd(r#"{"type":"select_dharma_sutra","title":"心经"}"#);
        let started = dispatch_cmd(r#"{"type":"submit_composer"}"#);
        let rev0 = started["ui_revision"].as_u64().unwrap();
        let t1: serde_json::Value = serde_json::from_str(&unsafe {
            take_result(fabushi_wasm_tick(250.0, 1_250.0, 420.0, 780.0))
        })
        .unwrap();
        assert_eq!(t1["ui_revision"].as_u64().unwrap(), rev0);

        // Trigger a steady toast (already-locked or missing practice) and confirm tick keeps ui_revision steady
        let _ = dispatch_cmd(r#"{"type":"select_tab","tab":"zen_room"}"#);
        let locked = dispatch_cmd(r#"{"type":"select_and_lock_practice","title":"金刚经"}"#);
        let rev_toast = locked["ui_revision"].as_u64().unwrap();
        let _ = dispatch_cmd(r#"{"type":"start_practice_session"}"#);
        let rev_sess = serde_json::from_str::<serde_json::Value>(&unsafe {
            take_result(fabushi_wasm_tick(33.0, 1_283.0, 420.0, 780.0))
        })
        .unwrap()["ui_revision"]
            .as_u64()
            .unwrap();
        assert!(rev_sess >= rev_toast);
        // Advancing > 1 second during Zen session must NOT bump ui_revision
        let t_sec: serde_json::Value = serde_json::from_str(&unsafe {
            take_result(fabushi_wasm_tick(250.0, 2_500.0, 420.0, 780.0))
        })
        .unwrap();
        let t_sec2: serde_json::Value = serde_json::from_str(&unsafe {
            take_result(fabushi_wasm_tick(250.0, 2_750.0, 420.0, 780.0))
        })
        .unwrap();
        let t_sec3: serde_json::Value = serde_json::from_str(&unsafe {
            take_result(fabushi_wasm_tick(250.0, 3_000.0, 420.0, 780.0))
        })
        .unwrap();
        let t_sec4: serde_json::Value = serde_json::from_str(&unsafe {
            take_result(fabushi_wasm_tick(250.0, 3_250.0, 420.0, 780.0))
        })
        .unwrap();
        assert_eq!(t_sec["ui_revision"], rev_sess);
        assert_eq!(t_sec2["ui_revision"], rev_sess);
        assert_eq!(t_sec3["ui_revision"], rev_sess);
        assert_eq!(t_sec4["ui_revision"], rev_sess);
        assert_eq!(t_sec4["snapshot"]["session_elapsed_secs"], 1);
    }
}
