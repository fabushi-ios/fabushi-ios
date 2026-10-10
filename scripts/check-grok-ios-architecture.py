#!/usr/bin/env python3
import argparse
import csv
import json
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_REFERENCE = "a9f633e09d49a85829b8236331b9e21f7e612634"
EXPECTED_FILES = 2046
VALID_STATUSES = {"mapped", "implemented", "verified", "not-applicable"}

parser = argparse.ArgumentParser()
parser.add_argument("--strict", action="store_true")
parser.add_argument("--complete", action="store_true", help="require every Grok row to be verified or explicitly not-applicable")
args = parser.parse_args()

errors = []
warnings = []

manifest_path = ROOT / "manifests/grok-bot-0.18-reference-files.json"
ledger_path = ROOT / "docs/parity/grok-bot-0.18-ios-parity-ledger.csv"
manifest = json.loads(manifest_path.read_text())
if manifest["reference"]["commit"] != EXPECTED_REFERENCE:
    errors.append("reference commit drift")
if manifest["fileCount"] != EXPECTED_FILES or len(manifest["files"]) != EXPECTED_FILES:
    errors.append("reference manifest is not complete")

with ledger_path.open(newline="", encoding="utf-8") as handle:
    rows = [row for row in csv.DictReader(line for line in handle if not line.startswith("#"))]
if len(rows) != EXPECTED_FILES:
    errors.append(f"parity ledger has {len(rows)} rows, expected {EXPECTED_FILES}")
paths = [row["grok_path"] for row in rows]
if len(set(paths)) != EXPECTED_FILES:
    errors.append("parity ledger contains duplicate Grok paths")
if set(paths) != {row["path"] for row in manifest["files"]}:
    errors.append("ledger and pinned reference manifest differ")

status_counts = Counter()
materialized = 0
for row in rows:
    status = row["implementation_status"].strip()
    target = row["ios_target_path"].strip()
    target_exists = bool(target) and (ROOT / target).is_file()
    status_counts[status] += 1
    materialized += int(target_exists)

    if status not in VALID_STATUSES:
        errors.append(f"{row['grok_path']}: invalid implementation_status={status!r}")
        continue
    if status in {"implemented", "verified"} and not target_exists:
        errors.append(f"{row['grok_path']}: {status} target is missing: {target}")
    if status == "verified" and not row["test_evidence"].strip():
        errors.append(f"{row['grok_path']}: verified row has no test_evidence")
    if status == "not-applicable" and not row["adaptation_reason"].strip():
        errors.append(f"{row['grok_path']}: not-applicable row has no adaptation_reason")
    if args.complete and status not in {"verified", "not-applicable"}:
        errors.append(f"{row['grok_path']}: completion gate still has status={status}")

required = [
    "frontend/src/production/ProductionRenderer.view.swift",
    "frontend/src/production/FabushiSceneRoot.swift",
    "frontend/src/production/GrokMobileShell.swift",
    "frontend/src/production/GrokMobileShell+Semantic.swift",
    "frontend/src/production/GrokMobileShell+Home.swift",
    "frontend/src/production/GrokMobileShell+Bots.swift",
    "frontend/src/production/GrokMobileBotService.swift",
    "frontend/src/recovered/features/app-shell/ContentView.swift",
    "source/ios-main/IOSMainRuntime.swift",
    "source/ios-main/dev/dev-capability.swift",
    "source/ios-main/dev/dev-controls-gate.swift",
    "source/ios-main/dev/dev-gateway-offline.swift",
    "source/ios-main/dev/dev-network-latency.swift",
    "source/ios-main/account/cursor-machine-id.swift",
    "source/ios-main/downloads/download-path.swift",
    "source/ios-main/background-transfer/ios-background-transfer-service.swift",
    "source/ios-main/adapters/ios-native-local-capability-backend.swift",
    "source/ios-main/auth/ios-passkey-provider.swift",
    "source/ios-main/deep-link/deep-link-controller.swift",
    "source/ios-main/lifecycle/ios-lifecycle-recovery.swift",
    "source/ios-main/telemetry/desktop-lifecycle-telemetry.swift",
    "source/ios-main/telemetry/sentry-conversation.swift",
    "source/ios-main/notifications/dock-badge.swift",
    "source/ios-main/telemetry/send-telemetry.swift",
    "source/ios-main/process-metrics/redaction.swift",
    "source/ios-main/onepassword/onepassword-provisioning-contract.swift",
    "source/ios-main/FabushiRuntime.swift",
    "source/ios-preload/preload.swift",
    "source/ios-preload/coordinator-port-bridge.swift",
    "source/ios-preload/box-vnc-clipboard-paste.swift",
    "source/ios-preload/box-vnc-liveness.swift",
    "source/ios-preload/box-vnc-visibility-gate.swift",
    "source/ios-preload/main-rpc-runtime.swift",
    "source/ios-preload/passkey-stall.swift",
    "source/ios-preload/preload-browser-base.swift",
    "source/ios-preload/preload-dev-controls.swift",
    "source/ios-preload/preload-vnc.swift",
    "source/ios-preload/preload-webview.swift",
    "source/ios-preload/rpc-edge-runtime.swift",
    "source/ios-preload/runtime/DevControlsPreloadEntrypoint.swift",
    "source/ios-preload/runtime/primary.swift",
    "source/ios-preload/runtime/VNCPreloadEntrypoint.swift",
    "source/ios-preload/runtime/webview.swift",
    "source/local-exec-daemon/invariant-violation-log.swift",
    "source/local-exec-daemon/production-executor.swift",
    "source/box-exec-daemon/cli.swift",
    "source/box-exec-daemon/server.swift",
    "source/mime-types.types.swift",
    "source/internal/host-extensions.rs",
    "source/internal/scheduling.rs",
    "source/shared/media/attachment-limits.swift",
    "source/shared/media/attachment-open-policy.swift",
    "source/shared/media/attachment-preview.swift",
    "source/shared/media/attachment-summary.swift",
    "source/shared/media/attachments.swift",
    "source/shared/media/avatar-image.swift",
    "source/shared/media/file-preview-kind.swift",
    "source/shared/media/image-dimensions.swift",
    "source/shared/media/image-mime.swift",
    "source/shared/media/markdown-preview.swift",
    "source/shared/media/media-extensions.swift",
    "source/shared/media/video-dimensions.swift",
    "source/shared/desktop.swift",
    "source/shared/deep-link.swift",
    "source/shared/external-url-policy.swift",
    "source/shared/link-preview-policy.swift",
    "source/shared/retry-after.swift",
    "source/shared/product-name.swift",
    "source/shared/update-track.swift",
    "source/shared/timezone.swift",
    "source/shared/system-errno.swift",
    "source/shared/vnc-liveness.swift",
    "source/shared/vnc-viewer-visibility.swift",
    "source/shared/message-reference.swift",
    "source/shared/ordering.swift",
    "source/shared/send-acceptance.swift",
    "source/shared/send-message-preview.swift",
    "source/shared/sidebar-sections.swift",
    "source/shared/sand-text.swift",
    "source/shared/usage.swift",
    "source/shared/write-epoch.swift",
    "source/ios-dev-controls/IOSDevControls.swift",
    "source/mahayana-agent-coordinator/MahayanaCoordinator.swift",
    "source/mahayana-agent-coordinator/local-host-supervisor.swift",
    "source/mahayana-agent-coordinator/renderer-port-server.swift",
    "source/mahayana-agent-coordinator/control-port-client.swift",
    "source/host/MahayanaHostRuntime.swift",
    "source/host/host-paths.rs",
    "source/host/process-crash-guard.rs",
    "source/host/notify-drain-gate.rs",
    "source/host/mcp-auth/mcp-auth-wait-registry.rs",
    "source/local-exec-daemon/LocalCapabilityRunner.swift",
    "source/box-exec-daemon/RemoteRunner.swift",
    "source/shared/rpc/coordinator-port.swift",
    "source/shared/rpc/coordinator.swift",
    "source/shared/rpc/SharedRPCContracts.swift",
    "source/packages/agent/state-utils.rs",
    "source/packages/agent/constants.rs",
    "source/packages/agent/state-agent-type.rs",
    "source/packages/agent/common.rs",
    "source/packages/agent/self-summary/constants.rs",
    "source/packages/agent/tools/task-tool-name.rs",
    "source/packages/cursor-plugins/schema-version.rs",
    "source/packages/cursor-plugins/validate-subpath.rs",
    "source/packages/cursor-plugins/environment-filter.rs",
    "source/packages/agent/tools/tool-execution-timeout.rs",
    "source/packages/agent/tools/core/read/common.rs",
    "source/packages/local-exec/services/team-settings-service.rs",
    "source/packages/agent/actions/background-shell-action-handler.rs",
    "source/packages/agent/actions/background-subagent-action-handler.rs",
    "source/packages/agent/actions/cancel-action-handler.rs",
    "source/packages/shell-exec/event-loop-pressure.swift",
    "source/packages/shell-exec/types.swift",
    "source/packages/shell-exec/output-suppression.swift",
    "source/packages/shell-exec/env-filter.swift",
    "source/packages/shell-exec/sandbox/network-policy-utils.swift",
    "mobile/ios/FabushiTests/PackageShellPolicyParityTests.swift",
    "source/packages/agent/utils/request-path.rs",
    "source/packages/agent/tools/lenient-boolean.rs",
    "source/packages/cursor-plugins/identifiers.rs",
    "source/packages/cursor-plugins/secret-variable-names.rs",
    "source/packages/agent/tools/lenient-enum.rs",
    "source/packages/agent/context-processing-skill-id.rs",
    "source/packages/agent/utils/meta-parent-completion-protocol.rs",
    "source/packages/agent/utils/mcp-auth-instruction.rs",
    "source/packages/agent/prompts/anti-ask-question-copy.rs",
    "source/packages/agent/prompts/claude-helpers.rs",
    "source/packages/agent/prompts/cloud/no-repository-access.rs",
    "source/packages/agent/utils/slack-sender-line.rs",
    "source/packages/local-exec/shell-timeout.rs",
    "source/packages/agent/prompts/user-info-sanitization.rs",
    "source/packages/agent/context-processing-uploaded-documents.rs",
    "source/packages/agent/utils/agent-mode-guidance.rs",
    "source/packages/agent/tools/core/read/pdf-utils.rs",
    "source/packages/cursor-plugins/cloud-manifest.rs",
    "source/packages/agent/context-processing-invocation.rs",
    "source/packages/hooks-carriers/hook-additional-context-render.rs",
    "source/packages/hooks-carriers/errors.rs",
    "source/packages/agent/context-processing-cursor-commands.rs",
    "source/packages/agent/context-processing-documentation.rs",
    "source/packages/agent-store-sync/sync-client-config.rs",
    "source/packages/context/browser-bridge.rs",
    "source/packages/hooks/sanitize-system-reminder.rs",
    "source/packages/hooks/hook-step.rs",
    "source/packages/local-exec/int32.rs",
    "source/packages/local-exec/mcp.rs",
    "source/packages/local-exec/mcp-tool-annotations.rs",
    "source/packages/hooks/validators/base.rs",
    "source/packages/hooks/validators/baseHookResponse.rs",
    "source/packages/hooks/validators/afterAgentResponseResponse.rs",
    "source/packages/hooks/validators/afterAgentThoughtResponse.rs",
    "source/packages/hooks/validators/afterEditFileResponse.rs",
    "source/packages/hooks/validators/afterMCPExecutionResponse.rs",
    "source/packages/hooks/validators/afterShellExecutionResponse.rs",
    "source/packages/hooks/validators/afterTabFileEditResponse.rs",
    "source/packages/hooks/validators/sessionEndResponse.rs",
    "source/packages/hooks/validators/postToolUseFailureResponse.rs",
    "source/packages/hooks/validators/postToolUseResponse.rs",
    "source/packages/hooks/validators/stopResponse.rs",
    "source/packages/hooks/validators/subagentStopResponse.rs",
    "source/packages/hooks/validators/preCompactResponse.rs",
    "source/packages/hooks/validators/subagentStartResponse.rs",
    "source/packages/hooks/validators/workspaceOpenResponse.rs",
    "source/packages/hooks/validators/beforeReadFileResponse.rs",
    "source/packages/hooks/validators/beforePromptSubmitResponse.rs",
    "source/packages/hooks/validators/beforeTabFileReadResponse.rs",
    "source/packages/hooks/validators/beforeCommandExecutionHookResponse.rs",
    "source/packages/hooks/validators/sessionStartResponse.rs",
    "source/packages/hooks/validators/preToolUseResponse.rs",
    "mobile/ios/FabushiTests/DevControlsParityTests.swift",
    "mobile/ios/FabushiTests/IOSMainPolicyParityTests.swift",
    "mobile/ios/FabushiTests/IOSPlatformMainParityTests.swift",
]
for relative in required:
    if not (ROOT / relative).is_file():
        errors.append(f"missing architecture root file: {relative}")

preload = (ROOT / "source/ios-preload/preload.swift").read_text()
for forbidden in ["CoordinatorControlPortClient", "main.dispatch("]:
    if forbidden in preload:
        errors.append(f"iOS preload bypasses renderer coordinator-port boundary: {forbidden}")
if "IOSCoordinatorPortClient" not in preload:
    errors.append("iOS preload does not use its renderer-facing coordinator-port client")




background_transfer = (ROOT / "source/ios-main/background-transfer/ios-background-transfer-service.swift").read_text()
for required_token in [
    "URLSessionConfiguration.background",
    "sessionSendsLaunchEvents = true",
    "handleEvents(",
    "urlSessionDidFinishEvents",
]:
    if required_token not in background_transfer:
        errors.append(f"background-transfer lifecycle is incomplete: {required_token}")

passkey_provider = (ROOT / "source/ios-main/auth/ios-passkey-provider.swift").read_text()
for required_token in [
    "ASAuthorizationPlatformPublicKeyCredentialProvider",
    "ASAuthorizationController",
    "allowedRelyingPartyIDs",
]:
    if required_token not in passkey_provider:
        errors.append(f"native iOS passkey provider is incomplete: {required_token}")

coordinator_runtime = (ROOT / "source/mahayana-agent-coordinator/MahayanaCoordinator.swift").read_text()
for required_token in [
    "MahayanaLocalHostSupervisor",
    "observedHostGeneration",
    "recoverAfterFailure",
    "CoordinatorDevControlAdapting",
    "devControlAdapter.route(method: method, params: params)",
    "devControlAdapter.beforeProductionRequest()",
]:
    if required_token not in coordinator_runtime:
        errors.append(f"MahayanaCoordinator is missing fail-closed Host recovery: {required_token}")

mobile_host_crate = (ROOT / "source/packages/mahayana-rs/mahayana-app-host-mobile/src/lib.rs").read_text()
if 'host/host-paths.rs' not in mobile_host_crate:
    errors.append("shipping mobile Host crate does not compile source/host/host-paths.rs")
for required_token in [
    'agent/state-agent-type.rs',
    'agent/common.rs',
    'agent/context-processing-documentation.rs',
    'hooks-carriers/errors.rs',
    'local-exec/mcp-tool-annotations.rs',
]:
    if required_token not in mobile_host_crate:
        errors.append(f"shipping mobile Host crate does not compile package parity module: {required_token}")

host_paths = (ROOT / "source/host/host-paths.rs").read_text()
for required_token in [
    "ensure_data_root_alias",
    "resolve_sand_user_data_dir",
    "reanchor_sand_path_with",
    "get_gateway_discovery_path",
    "get_host_lock_path",
    "get_host_secrets_path",
    "get_host_upgrade_marker_path",
    "get_host_crash_marker_path",
]:
    if required_token not in host_paths:
        errors.append(f"host-paths parity is incomplete: {required_token}")

local_host_supervisor = (ROOT / "source/mahayana-agent-coordinator/local-host-supervisor.swift").read_text()
for required_token in [
    "observedGeneration == generation",
    "recoveryCount",
    "factory()",
]:
    if required_token not in local_host_supervisor:
        errors.append(f"local Host supervisor is missing generation recovery: {required_token}")

runtime = (ROOT / "source/ios-main/FabushiRuntime.swift").read_text()
for required_token in ["IOSDeepLinkController", "resyncAfterLifecycleRecovery", "resumeAfterBackground"]:
    if required_token not in runtime:
        errors.append(f"FabushiRuntime is missing production lifecycle/deep-link integration: {required_token}")

ios_main = (ROOT / "source/ios-main/IOSMainRuntime.swift").read_text()
for required_token in ["IOSLifecycleRecoveryStore", "lifecycleReporter", "markResyncCompleted", "IOSNativeDevControlAdapter", "devControlAdapter: devControlAdapter"]:
    if required_token not in ios_main:
        errors.append(f"IOSMainRuntime is missing lifecycle recovery integration: {required_token}")

dev_capability = (ROOT / "source/ios-main/dev/dev-capability.swift").read_text()
for required_token in ["FABUSHI_DEV_CAPABILITY", "preloadKind", "#if DEBUG"]:
    if required_token not in dev_capability:
        errors.append(f"iOS dev capability is incomplete: {required_token}")

dev_gate = (ROOT / "source/ios-main/dev/dev-controls-gate.swift").read_text()
for required_token in ["IOSNativeDevControlAdapter", "beforeProductionRequest", "dev.setGatewayOffline", "dev.setNetworkLatency"]:
    if required_token not in dev_gate:
        errors.append(f"iOS dev controls gate/wiring is incomplete: {required_token}")

dev_offline = (ROOT / "source/ios-main/dev/dev-gateway-offline.swift").read_text()
for required_token in ["requireOnline", "reapplyAfterCoordinatorLaunch", "MainActor"]:
    if required_token not in dev_offline:
        errors.append(f"iOS gateway-offline control is incomplete: {required_token}")

dev_latency = (ROOT / "source/ios-main/dev/dev-network-latency.swift").read_text()
for required_token in ["maximumMilliseconds = 10_000", "applyBeforeProductionRequest", "Task<Never, Never>.sleep"]:
    if required_token not in dev_latency:
        errors.append(f"iOS network-latency control is incomplete: {required_token}")

coordinator_restart = (ROOT / "source/ios-main/coordinator/coordinator-runtime.swift").read_text()
if coordinator_restart.count("coordinatorDidLaunchForDevControls()") < 2:
    errors.append("coordinator relaunch does not reapply native developer-control state")

preload_dev = (ROOT / "source/ios-preload/preload-dev-controls.swift").read_text()
for required_token in ["dev.gateway.offline", "dev.setNetworkLatency", "dev.networkLatencyStatus"]:
    if required_token not in preload_dev:
        errors.append(f"dev-controls preload is missing native network wiring: {required_token}")

runtime_dev_preload = (ROOT / "source/ios-preload/runtime/DevControlsPreloadEntrypoint.swift").read_text()
if "installIfEnabled" not in runtime_dev_preload or "capability.preloadKind == .devControls" not in runtime_dev_preload:
    errors.append("dev-capability does not gate the iOS developer-controls preload")

platform_main_tests = (ROOT / "mobile/ios/FabushiTests/IOSPlatformMainParityTests.swift").read_text()
for required_token in [
    "testSentryConversationReportRequiresNullOrBoundedAgentId",
    "testDockBadgeMatchesRecoveredUnreadRulesAndAppliesNativeBadgeCount",
    "testSendTelemetryValidationAndProjectionMatchRecoveredContract",
    "testProcessNameRedactionPreservesOnlyKnownHelperLabels",
    "testUnavailableOnePasswordProvisioningSinkFailsClosed",
    "IOSDockBadgeController",
]:
    if required_token not in platform_main_tests:
        errors.append(f"iOS platform-main parity XCTest evidence is incomplete: {required_token}")

sentry_conversation = (ROOT / "source/ios-main/telemetry/sentry-conversation.swift").read_text()
if "isSandSentryBoundedTagValue" not in sentry_conversation:
    errors.append("sentry-conversation does not use the shared bounded-tag privacy contract")

dock_badge = (ROOT / "source/ios-main/notifications/dock-badge.swift").read_text()
for required_token in [
    "computeDockBadgeTotal",
    "UNUserNotificationCenter.current().setBadgeCount",
    "IOSDockBadgeController",
]:
    if required_token not in dock_badge:
        errors.append(f"iOS dock-badge adaptation is incomplete: {required_token}")

ios_main_policy_tests = (ROOT / "mobile/ios/FabushiTests/IOSMainPolicyParityTests.swift").read_text()
for required_token in [
    "testDownloadPathPolicyPreservesSafeBasenamesAndAbsoluteOverride",
    "testMachineIDResolverDoubleChecksAfterEncryptedStorageSettles",
    "testMachineIDResolverPersistsGeneratedIDWhenStillMissing",
]:
    if required_token not in ios_main_policy_tests:
        errors.append(f"iOS main policy parity XCTest evidence is incomplete: {required_token}")

dev_control_tests = (ROOT / "mobile/ios/FabushiTests/DevControlsParityTests.swift").read_text()
for required_token in [
    "DevControlsProductionTestHost",
    "testCoordinatorProductionRequestAppliesOfflineAndLatencyBeforeHost",
    "MahayanaCoordinator(",
    "XCTAssertTrue(host.methods.isEmpty)",
    'XCTAssertEqual(host.methods, ["listAgents"])',
]:
    if required_token not in dev_control_tests:
        errors.append(f"dev-control production-path XCTest evidence is incomplete: {required_token}")

app = (ROOT / "mobile/ios/Fabushi/FabushiApp.swift").read_text()
for forbidden in ["MahayanaHost", "MahayanaCoordinator", "MarketplaceModel(", "MessagingModel("]:
    if forbidden in app:
        errors.append(f"FabushiApp owns runtime responsibility: {forbidden}")

if "FabushiSceneRoot()" not in app:
    errors.append("FabushiApp does not delegate scene composition to FabushiSceneRoot")
for forbidden in [".task", ".onChange", ".onOpenURL", "NotificationCenter", "FabushiRuntime("]:
    if forbidden in app:
        errors.append(f"FabushiApp still owns scene/runtime orchestration: {forbidden}")
if len(app.splitlines()) > 24:
    errors.append("FabushiApp has regrown beyond thin App/Scene composition")

shell_path = ROOT / "frontend/src/production/GrokMobileShell.swift"
shell = shell_path.read_text()
if len(shell.splitlines()) > 120:
    errors.append("GrokMobileShell has regrown into a monolithic renderer/runtime file")
for shell_part in (ROOT / "frontend/src/production").glob("GrokMobileShell*.swift"):
    shell_part_text = shell_part.read_text()
    for forbidden in ["bridge.request(", "feature.execute", "feature.receive"]:
        if forbidden in shell_part_text:
            errors.append(
                f"SwiftUI shell owns protocol/runtime I/O ({forbidden}): "
                f"{shell_part.relative_to(ROOT)}"
            )
if "appAgentSurface.publish(" in shell:
    errors.append("GrokMobileShell main file owns semantic-surface publication")

for path in (ROOT / "mobile/ios/Fabushi").glob("*.swift"):
    if path.name == "MahayanaHost.swift":
        continue
    text = path.read_text()
    if "MahayanaHost" in text:
        errors.append(f"presentation/platform source bypasses coordinator through Host: {path.relative_to(ROOT)}")

for root in ["source/box-exec-daemon", "source/local-exec-daemon", "source/packages/shell-exec"]:
    for path in (ROOT / root).rglob("*.swift"):
        text = path.read_text()
        for forbidden in ["Process(", "NSTask", "posix_spawn", "/bin/sh", "/bin/bash"]:
            if forbidden in text:
                errors.append(f"iOS runner emulates forbidden desktop process semantics ({forbidden}): {path.relative_to(ROOT)}")

for root in ["source/box-exec-daemon", "source/local-exec-daemon", "source/host", "source/packages/shell-exec"]:
    for path in (ROOT / root).rglob("*.swift"):
        text = path.read_text()
        if "IOSPreloadBridge" in text:
            errors.append(f"lower runtime layer depends on renderer preload bridge: {path.relative_to(ROOT)}")

shell_env_filter = (ROOT / "source/packages/shell-exec/env-filter.swift").read_text()
for required_token in [
    "ELECTRON_RUN_AS_NODE",
    "SSH_AUTH_SOCK",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "WAYLAND_DISPLAY",
    "sanitizeRemoteRunnerParams",
]:
    if required_token not in shell_env_filter:
        errors.append(f"shell environment filter is incomplete: {required_token}")

production_local_exec = (ROOT / "source/local-exec-daemon/production-executor.swift").read_text()
if "ShellExecEnvironmentFilter.sanitizeRemoteRunnerParams(params)" not in production_local_exec:
    errors.append("production Remote Runner path bypasses shell environment sanitization")

network_policy = (ROOT / "source/packages/shell-exec/sandbox/network-policy-utils.swift").read_text()
for required_token in [
    "networkDisabledPolicy",
    "networkAllowAllPolicy",
    "isNetworkEnabled",
    "defaultAction == .allow",
]:
    if required_token not in network_policy:
        errors.append(f"shell network-policy contract is incomplete: {required_token}")

reviewed_desktop_only_not_applicable = {
    "source/electron-main/startup/move-to-applications-folder.ts",
    "source/electron-main/update/win32-installer.ts",
    "source/electron-main/process-metrics/wiring.ts",
    "source/electron-main/update/update-gate.ts",
    "source/electron-main/update/safe-relaunch-gate.ts",
}
for grok_path in reviewed_desktop_only_not_applicable:
    row = next((row for row in rows if row["grok_path"] == grok_path), None)
    if row is None:
        errors.append(f"reviewed desktop-only parity row is missing: {grok_path}")
        continue
    if row["implementation_status"] != "not-applicable":
        errors.append(f"desktop-only row must remain reviewed not-applicable: {grok_path}")
    if "desktop-only" not in row["adaptation_reason"].lower():
        errors.append(f"desktop-only row is missing reviewed rationale: {grok_path}")

reviewed_barrel_not_applicable = {
    "source/packages/context/index.ts",
    "source/packages/hooks-carriers/index.ts",
    "source/packages/shell-exec/index.ts",
    "source/packages/chat-inference-proto/index.ts",
    "source/packages/local-exec/index.ts",
    "source/electron-main/adapters/index.ts",
}
for grok_path in reviewed_barrel_not_applicable:
    row = next((row for row in rows if row["grok_path"] == grok_path), None)
    if row is None:
        errors.append(f"reviewed re-export parity row is missing: {grok_path}")
        continue
    if row["implementation_status"] != "not-applicable":
        errors.append(f"pure re-export row must remain reviewed not-applicable: {grok_path}")
    if "re-export" not in row["adaptation_reason"].lower():
        errors.append(f"pure re-export row is missing reviewed rationale: {grok_path}")

unsafe_spawn_row = next(
    (row for row in rows if row["grok_path"] == "source/packages/shell-exec/sandbox/unsafe-spawn.ts"),
    None,
)
if unsafe_spawn_row is None:
    errors.append("unsafe-spawn parity row is missing")
elif unsafe_spawn_row["implementation_status"] != "not-applicable":
    errors.append("unsafe-spawn must remain reviewed not-applicable on iOS")
elif "process spawn" not in unsafe_spawn_row["adaptation_reason"].lower():
    errors.append("unsafe-spawn not-applicable review is missing the iOS process-spawn rationale")

# Swift requires source basenames to be unique inside one compilation target.
# Grok's repeated main.ts/view.tsx names are mapped to semantic iOS filenames
# unless/until those folders become separate Swift modules.
compiled_roots = [
    ROOT / "mobile/ios/Fabushi",
    ROOT / "frontend",
    ROOT / "source/internal",
    ROOT / "source/shared",
    ROOT / "source/ios-dev-controls",
    ROOT / "source/ios-main",
    ROOT / "source/ios-preload",
    ROOT / "source/mahayana-agent-coordinator",
    ROOT / "source/local-exec-daemon",
    ROOT / "source/box-exec-daemon",
]
by_basename = defaultdict(list)
for root in compiled_roots:
    if not root.exists():
        continue
    for path in root.rglob("*.swift"):
        by_basename[path.name].append(path.relative_to(ROOT))
for basename, duplicates in sorted(by_basename.items()):
    if len(duplicates) > 1:
        errors.append(
            f"Swift filename collision in app target ({basename}): "
            + ", ".join(str(path) for path in duplicates)
        )

project = (ROOT / "mobile/ios/project.yml").read_text()
for required_source in [
    "../../frontend",
    "../../source/internal",
    "../../source/mime-types.types.swift",
    "../../source/shared",
    "../../source/ios-dev-controls",
    "../../source/ios-main",
    "../../source/ios-preload",
    "../../source/mahayana-agent-coordinator",
    "../../source/host/MahayanaHostRuntime.swift",
    "../../source/packages/shell-exec/event-loop-pressure.swift",
    "../../source/packages/shell-exec/types.swift",
    "../../source/packages/shell-exec/output-suppression.swift",
    "../../source/packages/shell-exec/env-filter.swift",
    "../../source/packages/shell-exec/sandbox/network-policy-utils.swift",
]:
    if required_source not in project:
        errors.append(f"XcodeGen target does not compile {required_source}")
if "$(SRCROOT)/Frameworks" in project:
    errors.append("opaque Frameworks runtime remains in production library path")

runtime_manifest = ROOT / "source/packages/mahayana-rs/Cargo.toml"
mobile_ffi = ROOT / "source/packages/mahayana-rs/mahayana-app-host-mobile/src/lib.rs"
internal_host_extensions = ROOT / "source/internal/host-extensions.rs"
internal_scheduling = ROOT / "source/internal/scheduling.rs"
if internal_host_extensions.is_file() and internal_scheduling.is_file() and mobile_ffi.is_file():
    mobile_ffi_text = mobile_ffi.read_text()
    for required_module in [
        "internal/host-extensions.rs",
        "internal/scheduling.rs",
        "cursor-plugins/snapshot-state.rs",
        "local-exec/pi/truncate.rs",
        "hooks-carriers/limits.rs",
        "mcp-core/config/mcp-focus-retry-cooldown.rs",
        "mcp-core/config/mcp-fsm-timing-config.rs",
        "mcp-core/config/mcp-inline-reconnect-cooldown.rs",
        "local-exec/constants.rs",
        "agent/utils/token-estimate.rs",
        "agent/utils/prompt-xml-escape.rs",
        "agent/utils/request-path.rs",
        "agent/tools/lenient-boolean.rs",
        "cursor-plugins/identifiers.rs",
        "cursor-plugins/secret-variable-names.rs",
        "agent/tools/lenient-enum.rs",
        "agent/context-processing-skill-id.rs",
        "agent/utils/meta-parent-completion-protocol.rs",
        "agent/utils/mcp-auth-instruction.rs",
        "agent/prompts/anti-ask-question-copy.rs",
        "agent/prompts/claude-helpers.rs",
        "agent/prompts/cloud/no-repository-access.rs",
        "agent/utils/slack-sender-line.rs",
        "local-exec/shell-timeout.rs",
        "agent/prompts/user-info-sanitization.rs",
        "agent/context-processing-uploaded-documents.rs",
        "agent/utils/agent-mode-guidance.rs",
        "agent/tools/core/read/pdf-utils.rs",
        "cursor-plugins/cloud-manifest.rs",
        "agent/context-processing-invocation.rs",
        "hooks-carriers/hook-additional-context-render.rs",
        "agent/context-processing-cursor-commands.rs",
        "agent/constants.rs",
        "agent-store-sync/sync-client-config.rs",
        "local-exec/pending-decision-provider.rs",
        "mcp-core/config/mcp-tool-call-timeout.rs",
        "local-exec/agent-data-cleanup.rs",
        "agent/prompts/user-info.rs",
        "local-exec/services/cloud-rules-service.rs",
        "local-exec/ignore-rules.rs",
        "cursor-plugins/plugin-variables.rs",
        "cursor-plugins/capabilities.rs",
        "hooks-carriers/collect.rs",
        "hooks/hook-step.rs",
        "hooks/validators/preToolUseResponse.rs",
        "agent/tools/core/worktree-paths.rs",
        "local-exec/team-repo-filters.rs",
        "agent/tools/subagent-model-force-policy.rs",
        "agent/tools/subagent-composer-model-ids.rs",
        "agent/self-summary/constants.rs",
        "agent/tools/task-tool-name.rs",
        "cursor-plugins/schema-version.rs",
        "cursor-plugins/validate-subpath.rs",
        "cursor-plugins/environment-filter.rs",
        "agent/tools/tool-execution-timeout.rs",
        "agent/tools/core/read/common.rs",
        "local-exec/services/team-settings-service.rs",
        "agent/actions/background-shell-action-handler.rs",
        "agent/actions/background-subagent-action-handler.rs",
        "agent/actions/cancel-action-handler.rs",
        "host/process-crash-guard.rs",
        "host/notify-drain-gate.rs",
        "host/mcp-auth/mcp-auth-wait-registry.rs",
        "host/agent-isolation/conversation-blob-db.rs",
        "host/agent-isolation/conversation-blob-gc.rs",
        "host/agent-isolation/legacy-blob-retirement.rs",
        "host/agent-isolation/conversation-blob-store.rs",
        "host/agent-isolation/agent-store-worker.rs",
        "host/agent-isolation/agent-worker-pool.rs",
        "host/agent-isolation/worker-blob-store.rs",
        "host/agent-isolation/transcript-mirror-offload.rs",
        "host/agent-isolation/transcript-mirror-worker.rs",
        "host/transcript-mirror/conversation-state-binary.rs",
        "host/transcript-mirror/generated-occurrence-codec.rs",
        "host/transcript-mirror/transcript-journal-codec.rs",
        "host/transcript-mirror/transcript-mirror-router.rs",
        "host/transcript-mirror/transcript-occurrence-deriver.rs",
        "host/transcript-mirror/transcript-mirror.rs",
        "host/transcript-mirror/legacy-transcript-mirror.rs",
        "host/transcript-mirror/production-provider.rs",
        "host/extensions/extension-ids.generated.rs",
        "host/extensions/session/session-diagnostics.rs",
        "host/extensions/telemetry/send-trace-sampler.rs",
        "host/host-diagnostics.rs",
        "host/ports/product-analytics.rs",
        "host/ports/sand-analytics-types.rs",
        "host/ports/transport.rs",
        "host/ports/user-computer.rs",
        "host/runner/clock-skew-guard.rs",
        "host/runner/video-container.rs",
        "host/sand-user-identity.rs",
        "host/selected-image-inputs.rs",
        "host/transcript-mutation-events.rs",
        "host/extensions/box-store-sync/files.rs",
        "host/extensions/local-exec/local-exec-failure-classifier.rs",
        "host/extensions/transcript/sand-automation-failure.rs",
        "host/runner/tools/mcp-server-resolution.rs",
        "host/extensions/box-store-sync/object-store-port.rs",
        "host/extensions/box-lifecycle/box-lifecycle-service.rs",
        "host/runner/site-visit-tracking.rs",
        "host/host-event-bus.rs",
        "host/extensions/turn-execution/turn-execution-service.rs",
        "host/extensions/turn-execution/extension.rs",
        "host/workflows/stat-keyed-parse-cache.rs",
        "host/attachment-paths.rs",
        "host/automations/automation-id.rs",
        "host/durable-file-policy.rs",
        "host/extensions/box-store-sync/box-store-diagnostics.rs",
        "host/extensions/box-store-sync/box-store-sync-error.rs",
        "host/extensions/cloud-agents/cloud-agent-launch-error.rs",
        "host/extensions/local-exec/local-exec-error.rs",
        "host/extensions/session/conversation-blobs-path.rs",
        "host/extensions/telemetry/auto-review-approval-telemetry.rs",
        "host/extensions/telemetry/automation-shadow-prune-telemetry.rs",
        "host/extensions/telemetry/box-log-ship-telemetry.rs",
        "host/extensions/telemetry/disk-pressure-telemetry.rs",
        "host/extensions/telemetry/experiments-diagnostic-telemetry.rs",
        "host/extensions/telemetry/host-diagnostic-telemetry.rs",
        "host/extensions/telemetry/host-event-bus-telemetry.rs",
        "host/extensions/telemetry/search-index-health-telemetry.rs",
        "host/extensions/telemetry/turn-empty-delivery-telemetry.rs",
        "host/extensions/transcript/channel-delivery-unregistered-error.rs",
        "host/extensions/transcript/send-not-persisted-error.rs",
        "host/runner/agent-state.rs",
        "host/runner/sand-prompt-markers.rs",
        "host/runner/tool-call-identity.rs",
        "host/runner/tools/sand-permission-request.rs",
        "host/runner/tools/sand-secret-request.rs",
        "host/runner/tools/tool-input-error.rs",
        "host/sha256.rs",
        "host/storage/folder-id.rs",
    ]:
        if required_module not in mobile_ffi_text:
            errors.append(f"iOS-owned Rust internal module is not compiled by mobile host: {required_module}")

    for required_token in [
        "MobileHostBridge",
        "MobileTurnExecutor",
        "bound_turn_execution_extension::<MobileHostBridge>",
        "start_host_extensions(",
        "turn_execution_registry",
    ]:
        if required_token not in mobile_ffi_text:
            errors.append(
                "mobile Host does not production-wire turn-execution: "
                + required_token
            )

    turn_execution_extension_text = (
        ROOT / "source/host/extensions/turn-execution/extension.rs"
    ).read_text()
    if "bound_turn_execution_extension" not in turn_execution_extension_text:
        errors.append(
            "turn-execution extension does not expose a production bound-executor declaration"
        )

legacy_runtime_import_workflow = ROOT / ".github/workflows/import-ios-owned-mahayana.yml"
if legacy_runtime_import_workflow.exists():
    errors.append(
        "legacy external Mahayana source import workflow remains; "
        "fabushi-ios must own its runtime source without a repository fallback"
    )

if not runtime_manifest.is_file() or not mobile_ffi.is_file():
    message = "iOS-owned Mahayana Rust source is missing from this repository"
    (errors if args.strict else warnings).append(message)

for legacy in [
    "mobile/ios/Fabushi/MahayanaHost.swift",
    "mobile/native/include/mahayana_app_host.h",
    "mobile/ios/Fabushi/GrokMobileShell.swift",
    "mobile/ios/Fabushi/ContentView.swift",
    "source/ios-main/main.swift",
    "source/ios-dev-controls/main.swift",
    "source/mahayana-agent-coordinator/main.swift",
    "source/local-exec-daemon/main.swift",
    "source/box-exec-daemon/main.swift",
    "source/shared/rpc/main.swift",
]:
    if args.strict and (ROOT / legacy).exists():
        errors.append(f"legacy or collision-prone source still exists: {legacy}")

print(
    "INFO: parity materialized="
    f"{materialized}/{EXPECTED_FILES} status="
    + ",".join(f"{key}:{status_counts[key]}" for key in sorted(status_counts))
)
if warnings:
    for warning in warnings:
        print(f"WARNING: {warning}")
if errors:
    for error in errors:
        print(f"ERROR: {error}")
    raise SystemExit(1)
print(
    f"PASS: Grok iOS architecture ledger={len(rows)} "
    f"reference={EXPECTED_REFERENCE} strict={args.strict} complete={args.complete}"
)
