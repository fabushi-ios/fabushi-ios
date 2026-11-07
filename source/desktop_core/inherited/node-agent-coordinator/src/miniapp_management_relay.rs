//! Native-only intent gate on the existing Coordinator main-data carrier.
use crate::carrier::CarrierChannel;

pub fn miniapp_native_method(method: &str) -> bool {
    matches!(
        method,
        "listApprovedMiniApps"
            | "listInstalledMiniApps"
            | "installApprovedMiniApp"
            | "getInstalledMiniAppResources"
            | "uninstallMiniApp"
    )
}

pub fn miniapp_carrier_allowed(method: &str, channel: CarrierChannel) -> bool {
    !miniapp_native_method(method) || channel == CarrierChannel::MainData
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_package_intents_and_resources_never_enter_from_renderer() {
        for method in [
            "listApprovedMiniApps",
            "listInstalledMiniApps",
            "installApprovedMiniApp",
            "getInstalledMiniAppResources",
            "uninstallMiniApp",
        ] {
            assert!(!miniapp_carrier_allowed(method, CarrierChannel::Data));
            assert!(!miniapp_carrier_allowed(method, CarrierChannel::Control));
            assert!(miniapp_carrier_allowed(method, CarrierChannel::MainData));
        }
        assert!(miniapp_carrier_allowed("listAgents", CarrierChannel::Data));
    }
}
