# Vị Trí Ảo (ViTriAo)

App iOS giả lập vị trí GPS cho cá nhân, **không cần jailbreak**. Chạy được với máy đã cài
qua **iloader** + **SideStore**.

App không "hack" iOS mà dùng đúng dịch vụ giả lập vị trí dành cho nhà phát triển
(giống Xcode). Chỉ khác là app tự kết nối tới **chính chiếc iPhone** qua một VPN loopback, bằng pairing file.

```
App (SwiftUI) ──C ABI──▶ locsim_core (Rust, thư viện idevice)
      │                        │  pairing file + IP 10.7.0.1 (LocalDevVPN)
      │                        ▼
      │              lockdownd → CoreDeviceProxy → RSD → DVT
      │                        → LocationSimulation (iOS 17+)
      └── Background location: giữ app chạy nền để kết nối không bị đóng
```

## Cần có

- iPhone **iOS 17 trở lên** (iOS 16 trở xuống cần mount Developer Disk Image, không khuyến nghị)
- **Developer Mode** đang bật: Cài đặt → Quyền riêng tư & Bảo mật → Chế độ nhà phát triển
- **SideStore** (cài bằng iloader) và app VPN **LocalDevVPN** (hoặc StosVPN) từ App Store
- **Pairing file** của chính iPhone đó (iloader tạo được)

## 1. Build IPA bằng Codemagic

1. Đưa thư mục này lên một repo GitHub/GitLab/Bitbucket (để private cũng được).
2. Vào codemagic.io → **Add application** → chọn repo → chọn kiểu project **Other**.
   Codemagic tự đọc `codemagic.yaml`.
3. Bấm **Start new build** → workflow **ViTriAo – IPA chưa ký**.
4. Build xong (khoảng 10–15 phút lần đầu), tải `ViTriAo.ipa` ở mục **Artifacts**.

Workflow làm 4 việc: cài Rust → build `core/` thành `liblocsim_core.a` cho `aarch64-apple-ios`
→ XcodeGen tạo `ViTriAo.xcodeproj` từ `project.yml` → `xcodebuild` không ký rồi nén thành IPA.

Nếu có Mac: chạy lần lượt `./scripts/build-core.sh`, `xcodegen generate`, `./scripts/build-ipa.sh`.

## 2. Cài lên iPhone

- **SideStore**: gửi `ViTriAo.ipa` sang iPhone (AirDrop, Files…), mở SideStore → My Apps → **+** → chọn file.
- Hoặc dùng iloader / Sideloadly trên máy tính để cài IPA với Apple ID.

Apple ID miễn phí: app hết hạn sau 7 ngày, SideStore sẽ tự làm mới (refresh) như các app khác.

## 3. Lấy pairing file

Trong iloader (cắm iPhone vào máy tính) → mục **Pairing File** → xuất/lưu file
(`.mobiledevicepairing` hoặc `.plist`), rồi gửi file sang iPhone.
Nếu SideStore đã chạy được thì bạn đã có sẵn pairing file. Có thể lấy lại bản đó qua app Tệp
(thư mục SideStore) rồi nhập vào app này.

Trong app: **Cài đặt** (bánh răng) → **Nhập pairing file…** → chọn file.
Cách khác: chép file vào thư mục *Vị Trí Ảo* trong app Tệp, app sẽ tự nhận.

## 4. Sử dụng

1. Bật **LocalDevVPN** (trạng thái Connected).
2. Mở Vị Trí Ảo, cho phép vị trí **Luôn luôn** (để app chạy nền giữ vị trí).
3. Chạm lên bản đồ, tìm địa điểm 🔍, hoặc dán toạ độ / link Google Maps (#).
4. Bấm **Bắt đầu**. Muốn đổi chỗ, chọn điểm khác rồi bấm **Dời tới đây**.
5. Bấm **Dừng** để về GPS thật. Khởi động lại iPhone cũng về GPS thật.

## Lỗi thường gặp

| Thông báo | Cách xử lý |
|---|---|
| Hết thời gian kết nối | VPN chưa bật hoặc IP sai (mặc định `10.7.0.1`) |
| Không mở được DVT | Chưa bật Developer Mode, hoặc pairing file không phải của máy này |
| Không đọc được pairing file | Nhập lại file từ iloader |
| Phiên giả lập đã bị ngắt | VPN bị tắt hoặc app bị iOS đóng. Bật lại VPN rồi bấm Bắt đầu |

## Cấu trúc

```
core/            Lõi Rust (staticlib) – src/lib.rs, include/locsim.h
App/Sources/     SwiftUI: bản đồ, tìm kiếm, yêu thích, cài đặt
App/Resources/   Icon
project.yml      Cấu hình XcodeGen
scripts/         build-core.sh, build-ipa.sh
codemagic.yaml   CI build IPA
```

Chỉ dùng cho mục đích cá nhân. Nhiều app/game cấm giả vị trí trong điều khoản sử dụng
và có thể khoá tài khoản.
