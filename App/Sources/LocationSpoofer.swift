import Foundation
import CoreLocation
import SwiftUI

/// Bọc thư viện Rust (locsim_core) và giữ trạng thái cho UI.
@MainActor
final class LocationSpoofer: ObservableObject {
    @Published private(set) var isActive = false
    @Published private(set) var isBusy = false
    @Published private(set) var spoofedCoordinate: CLLocationCoordinate2D?
    @Published var lastError: String?
    @Published private(set) var hasPairingFile = false

    @AppStorage("deviceIP") var deviceIP = "10.7.0.1"

    let keeper = BackgroundKeeper()

    static var documentsURL: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }
    static var pairingURL: URL { documentsURL.appendingPathComponent("pairingFile.plist") }

    init() {
        adoptDroppedPairingFile()
        refreshPairingState()
    }

    // MARK: Pairing file

    func refreshPairingState() {
        hasPairingFile = FileManager.default.fileExists(atPath: Self.pairingURL.path)
    }

    /// Nếu người dùng chép pairing file vào thư mục Documents của app (qua app Tệp / iloader)
    /// với tên khác, tự đổi tên thành pairingFile.plist.
    private func adoptDroppedPairingFile() {
        let fm = FileManager.default
        guard !fm.fileExists(atPath: Self.pairingURL.path),
              let items = try? fm.contentsOfDirectory(at: Self.documentsURL, includingPropertiesForKeys: nil)
        else { return }
        let candidate = items.first { url in
            let ext = url.pathExtension.lowercased()
            return ext == "mobiledevicepairing" || ext == "plist" || url.lastPathComponent.lowercased().contains("pairing")
        }
        if let candidate, Self.isValidPairing(candidate) {
            try? fm.copyItem(at: candidate, to: Self.pairingURL)
        }
    }

    static func isValidPairing(_ url: URL) -> Bool {
        guard let data = try? Data(contentsOf: url),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any]
        else { return false }
        return plist["HostCertificate"] != nil || plist["DeviceCertificate"] != nil || plist["HostPrivateKey"] != nil
    }

    func importPairingFile(from url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard Self.isValidPairing(url) else {
            lastError = "File này không phải pairing file hợp lệ (thiếu HostCertificate/HostPrivateKey)."
            return
        }
        do {
            let fm = FileManager.default
            if fm.fileExists(atPath: Self.pairingURL.path) { try fm.removeItem(at: Self.pairingURL) }
            try fm.copyItem(at: url, to: Self.pairingURL)
            lastError = nil
        } catch {
            lastError = "Không chép được pairing file: \(error.localizedDescription)"
        }
        refreshPairingState()
    }

    func deletePairingFile() {
        try? FileManager.default.removeItem(at: Self.pairingURL)
        refreshPairingState()
    }

    // MARK: Giả lập

    func start(at coordinate: CLLocationCoordinate2D) {
        guard hasPairingFile else {
            lastError = "Chưa có pairing file. Vào Cài đặt để nhập."
            return
        }
        guard !isBusy else { return }
        isBusy = true
        lastError = nil
        let path = Self.pairingURL.path
        let ip = deviceIP
        Task.detached(priority: .userInitiated) {
            let error = Self.callSet(path: path, ip: ip, coordinate: coordinate)
            await MainActor.run {
                self.isBusy = false
                if let error {
                    self.lastError = error
                    self.isActive = locsim_is_active() == 1
                } else {
                    self.isActive = true
                    self.spoofedCoordinate = coordinate
                    self.keeper.start()
                }
            }
        }
    }

    func stop() {
        guard !isBusy else { return }
        isBusy = true
        Task.detached(priority: .userInitiated) {
            var err: UnsafeMutablePointer<CChar>?
            let rc = locsim_clear(&err)
            let message = Self.takeString(err)
            await MainActor.run {
                self.isBusy = false
                self.isActive = false
                self.spoofedCoordinate = nil
                self.keeper.stop()
                if rc != 0 { self.lastError = message ?? "Tắt thất bại (mã \(rc))" }
            }
        }
    }

    /// Gọi khi app quay lại foreground để đồng bộ trạng thái thật của phiên.
    func syncState() {
        let active = locsim_is_active() == 1
        if isActive && !active {
            isActive = false
            spoofedCoordinate = nil
            keeper.stop()
            lastError = "Phiên giả lập đã bị ngắt (VPN tắt hoặc iOS đóng kết nối). Bấm Bắt đầu để kết nối lại."
        }
    }

    nonisolated private static func callSet(path: String, ip: String, coordinate: CLLocationCoordinate2D) -> String? {
        var err: UnsafeMutablePointer<CChar>?
        let rc = path.withCString { p in
            ip.withCString { i in
                locsim_set(p, i, coordinate.latitude, coordinate.longitude, &err)
            }
        }
        let message = takeString(err)
        return rc == 0 ? nil : (message ?? "Lỗi không xác định (mã \(rc))")
    }

    nonisolated private static func takeString(_ p: UnsafeMutablePointer<CChar>?) -> String? {
        guard let p else { return nil }
        defer { locsim_free_string(p) }
        return String(cString: p)
    }
}
