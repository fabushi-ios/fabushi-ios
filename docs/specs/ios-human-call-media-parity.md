# Native iOS Human Call media parity

Status: active  
Task ID: IOS-MAIN-HUMAN-CALL-MEDIA-001  
Last updated: 2026-10-08  
Target repository: `fabushi-ios/fabushi-ios`  
Canonical upstream: `bhrumom/fabushi-desktop@main`

## Authority and provenance

This specification governs the iOS adaptation of Desktop `frontend/src/production/human-call-media.tsx` at Desktop main `3bc92400826cc4ca7ac665b467708e22261edc61`, blob `8e5c4cbe5b3c9fb7f0d51d43300ccdeb2d2ac99e`. The live Desktop main and iOS pull-request head must be resolved again for every acceptance cycle. The canonical iOS migration specification and authority lock remain `docs/specs/fabushi-desktop-main-ios-parity.md` and `manifests/desktop-main-authority.json`.

## Context and problem

The iOS product already uses the canonical Host call-session, transport-lease, ICE and ordered signaling contracts and has a LiveKitWebRTC peer-connection owner. Before this task, the shipping Calls surface could create, accept, decline and end calls and could negotiate audio/video, but it did not render local or remote video, publish ReplayKit screen frames, or expose stable microphone/camera preference and stale-device fallback. That left the user-visible media lifecycle below Desktop-main parity even though call signaling existed.

## Goal

Extend the existing native Human Call owners so a user can conduct a voice or video call from the unified Fabushi product, see remote and local media, change microphone/camera devices with deterministic fallback, share the active Fabushi app through ReplayKit, recover an interrupted connection with ICE restart, and leave no media or device ownership behind after teardown.

## Non-goals and separately tracked work

- This task does not create a second Call app, identity store, conversation store, signaling protocol or peer-connection owner.
- Group-call topology, CallKit system-call presentation, push-to-wake, and background/suspended call continuation are separate responsibilities and remain open until their own upstream owners and native contracts are implemented.
- ReplayKit in-app capture is the shipping screen-share mechanism in this slice. Capturing other applications through a Broadcast Upload Extension remains a separate native lifecycle responsibility and cannot be claimed from this implementation.
- Passing ordinary and protected CI is not a substitute for a real two-device media journey; the row cannot become `verified` without that evidence.

## Existing owners and composition

- `HumanCallsView`: the single shipping SwiftUI call capability surface opened by the existing Fabushi shell.
- `HumanCallMediaPort`: AVFoundation permission, device enumeration, preference and audio-input owner.
- `HumanCallPeerConnection`: the single LiveKitWebRTC audio/video/screen-track, offer/answer/candidate and reconnect owner.
- Existing Coordinator/Host RPC methods: canonical call sessions, generation fencing, transport lease, ICE servers, media capability state and ordered signals.
- `HumanCallVideoView`: a renderer adapter only; it owns no call state and attaches a supplied `LKRTCVideoTrack` to the native renderer.

## Functional requirements

**IOS-CALL-FR-01 - Canonical ownership.** All media actions use the active Host call session and transport lease. No second signaling path, local-only call session, or parallel peer connection may be introduced.

**IOS-CALL-FR-02 - Rendered video.** The active call surface renders the remote video track and a local preview. Unified-plan receiver callbacks and legacy stream callbacks must converge on the same remote-track state. Teardown detaches renderers and clears both tracks.

**IOS-CALL-FR-03 - Stable video sender.** A disabled camera video sender is prepared before the first offer so later camera or screen-share activation does not require a second media owner. Camera and screen capture replace only the sender track while the canonical peer connection remains unchanged.

**IOS-CALL-FR-04 - Device preference and fallback.** Available microphones and cameras are enumerated with stable native IDs. The selected IDs are persisted in iOS-owned preferences. A missing or stale preferred device falls back deterministically to an available native device and updates the active selection. Microphone changes use `AVAudioSession.setPreferredInput`; camera changes restart capture on the same video source and track.

**IOS-CALL-FR-05 - ReplayKit screen share.** When ReplayKit reports availability, the user can start and stop active-app screen sharing. Video sample buffers are converted to WebRTC frames, published through a screen-cast source, and replace the current camera sender track. Stopping or failing capture restores the requested camera track or an intentionally blank disabled track. Screen-share state is projected through the existing `updateCallMedia` contract.

**IOS-CALL-FR-06 - Media controls.** Mute, camera and screen-share controls are accessible, reflect actual local state, and update Host media capabilities including selected device IDs. A denied permission or unavailable capture device fails closed and remains user visible.

**IOS-CALL-FR-07 - Recovery.** ICE disconnected/failed transitions drive the existing Host `reconnect` then `resume` state machine and create a new offer with ICE restart. Mute, requested camera state, screen-share intent and selected device preferences are restored when possible. A failed recovery transitions the call to terminal failure and tears media down.

**IOS-CALL-FR-08 - Lifecycle fencing.** Call ID, generation, transport device ID and ordered signal sequence remain mandatory. Switching calls, dismissing the surface, terminal call state, or application teardown stops camera and ReplayKit capture, closes the peer connection, deactivates the audio session and clears callbacks/renderers.

**IOS-CALL-FR-09 - Product and accessibility.** Media is presented inside the existing Calls capability surface with deterministic accessibility identifiers and labels. The user can always return to the unified Fabushi shell without losing canonical call-session truth.

## Data and control flow

1. `HumanCallsView` requests permissions and resolves persisted device preferences through `HumanCallMediaPort`.
2. The view obtains the Host transport lease and ICE configuration for the current call generation.
3. `HumanCallPeerConnection` prepares audio and a stable video sender, optionally starts the selected camera, and publishes local/remote track callbacks.
4. Offers, answers and candidates travel only through Host `sendCallSignal`/`listCallSignals`, fenced by call ID, generation, sender device and sequence.
5. Camera or ReplayKit changes replace the sender track and then update canonical Host media capability state.
6. Disconnect recovery reuses the Host lifecycle, recreates the peer connection with the same preferences and issues an ICE-restart offer.
7. Teardown clears native capture, renderer and audio-session ownership before the view or call is released.

## Failure modes

- Permission denied or restricted: do not create the affected track; show a user-facing error.
- Preferred device absent: select the deterministic fallback and continue; never fail solely because a stored ID is stale.
- Transport lease held by another device: fail closed before media capture.
- ReplayKit unavailable/start failure/sample failure: keep the call alive, restore camera/blank video, clear screen-share capability and show an error.
- Malformed ICE or signaling payload: reject it and preserve generation/sequence fencing.
- ICE recovery failure: mark the call failed with `media-reconnect-failed`, then release all native media resources.
- Terminal or switched call: ignore stale callbacks and signals from the previous media generation.

## Verification strategy

All executable verification runs in GitHub Actions. Required evidence for `implemented` is exact-head architecture, Rust Host, Swift unit/UI compilation/tests and device archive success. Unit tests cover permission normalization, device-preference persistence/resolution, stale-device fallback, call/session/lease/signal projections and screen-share media-state projection. Required evidence for `verified` additionally includes a real two-device voice/video journey with local/remote rendering, camera/microphone switching, screen-share start/stop, disconnect/reconnect with ICE restart, terminal teardown, screenshots/video and artifact provenance on the same exact iOS head.

## Acceptance criteria

- **IOS-CALL-AC-01:** FR-01 through FR-09 are implemented in the existing owners and shipping composition.
- **IOS-CALL-AC-02:** focused unit tests and the ordinary exact-head architecture/build/test/device-archive workflow pass without skipped required steps.
- **IOS-CALL-AC-03:** protected-account acceptance still passes on the same exact head and no canonical account/runtime behavior regresses.
- **IOS-CALL-AC-04:** a real two-device media journey proves remote/local rendering, device fallback/switching, ReplayKit start/stop, reconnect/ICE restart and teardown.
- **IOS-CALL-AC-05:** the parity ledger names exact production/test evidence and remains below `verified` until AC-04 and independent review pass.

## Release, rollback and observability

The change is delivered through PR #3 and inherits the app's normal rollback to the previous exact commit. User-visible media and signaling failures are surfaced in the call UI; Host call state, generation, terminal reason and media capability projection remain the durable diagnostic record. No build, test or release evidence from an earlier head may be promoted to the new head.
