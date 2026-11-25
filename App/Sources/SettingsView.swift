import SwiftUI
import UniformTypeIdentifiers
import CoreLocation

struct SettingsView: View {
    @EnvironmentObject private var spoofer: LocationSpoofer
    @Environment(\.dismiss) private var dismiss
    @State private var importing = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("Pairing file") {
                        Text(spoofer.hasPairingFile ? "Đã có" : "Chưa có")
                            .foregroundStyle(spoofer.hasPairingFile ? .green : .red)
                    }
                    Button(spoofer.hasPairingFile ? "Thay pairing file…" : "Nhập pairing file…") {
                        importing = true
                    }
                    if spoofer.hasPairingFile {
                        Button("Xoá pairing file", role: .destructive) { spoofer.deletePairingFile() }
                    }
                } header: {
                    Text("Pairing")
                } footer: {
                    Text("Dùng iloader trên máy tính: cắm iPhone → mục Pairing File → xuất file (thường tên .mobiledevicepairing hoặc .plist), gửi sang iPhone rồi chọn ở đây. Pairing file chỉ dùng cho đúng chiếc iPhone đã tạo ra nó.")
                }

                Section {
                    TextField("10.7.0.1", text: $spoofer.deviceIP)
                        .keyboardType(.numbersAndPunctuation)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("IP thiết bị (qua VPN)")
                } footer: {
                    Text("Bật LocalDevVPN (hoặc StosVPN) trước khi bấm Bắt đầu. Mặc định là 10.7.0.1.")
                }

                Section("Quyền vị trí") {
                    PermissionRow(keeper: spoofer.keeper)
                }

                Section("Checklist trước khi dùng") {
                    Label("iOS 17 trở lên (iOS 16 cần mount Developer Disk Image)", systemImage: "iphone")
                    Label("Cài đặt → Quyền riêng tư & Bảo mật → Chế độ nhà phát triển: BẬT", systemImage: "hammer")
                    Label("Đã nhập pairing file của máy này", systemImage: "doc.badge.gearshape")
                    Label("LocalDevVPN / StosVPN đang kết nối", systemImage: "network")
                    Label("Cho phép vị trí \"Luôn luôn\" để app chạy nền", systemImage: "location")
                }
                .font(.footnote)
            }
            .navigationTitle("Cài đặt")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Xong") { dismiss() } }
            }
            .fileImporter(isPresented: $importing,
                          allowedContentTypes: [.propertyList, .xml, .data, .item]) { result in
                if case .success(let url) = result { spoofer.importPairingFile(from: url) }
            }
            .safeAreaInset(edge: .bottom) {
                if let error = spoofer.lastError {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .padding()
                }
            }
        }
    }
}

private struct PermissionRow: View {
    @ObservedObject var keeper: BackgroundKeeper

    var body: some View {
        switch keeper.authorization {
        case .authorizedAlways:
            Label("Luôn luôn — app chạy nền được", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case .authorizedWhenInUse:
            Button("Nâng lên \"Luôn luôn\"") { keeper.requestPermission() }
        case .denied, .restricted:
            Button("Mở Cài đặt để cấp quyền vị trí") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
        default:
            Button("Cấp quyền vị trí") { keeper.requestPermission() }
        }
    }
}
