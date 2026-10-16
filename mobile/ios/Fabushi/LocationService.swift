import CoreLocation
import Observation

@MainActor
@Observable
final class LocationService: NSObject, CLLocationManagerDelegate {
    var coordinate: CLLocationCoordinate2D?
    var errorMessage: String?
    var loading = false

    private let manager = CLLocationManager()

    override init() {
        super.init()
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
    }

    func requestLocation() {
        coordinate = nil
        errorMessage = nil
        loading = true
        switch manager.authorizationStatus {
        case .notDetermined:
            manager.requestWhenInUseAuthorization()
        case .authorizedAlways, .authorizedWhenInUse:
            manager.requestLocation()
        case .denied, .restricted:
            loading = false
            errorMessage = "请在系统设置中允许位置权限"
        @unknown default:
            loading = false
            errorMessage = "无法读取位置授权状态"
        }
    }

    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let status = manager.authorizationStatus
        Task { @MainActor [weak self, status] in
            guard let self else { return }
            if status == .authorizedWhenInUse || status == .authorizedAlways {
                self.manager.requestLocation()
            } else if status == .denied || status == .restricted {
                loading = false
                errorMessage = "请在系统设置中允许位置权限"
            }
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        let nextCoordinate = locations.last?.coordinate
        Task { @MainActor [weak self, nextCoordinate] in
            guard let self else { return }
            loading = false
            coordinate = nextCoordinate
            if coordinate == nil { errorMessage = "暂时无法获取当前位置" }
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        let message = error.localizedDescription
        Task { @MainActor [weak self, message] in
            guard let self else { return }
            loading = false
            errorMessage = message
        }
    }
}
