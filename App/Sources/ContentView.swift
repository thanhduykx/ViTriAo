import SwiftUI
import MapKit

struct ContentView: View {
    @EnvironmentObject private var spoofer: LocationSpoofer
    @StateObject private var favorites = FavoritesStore()
    @Environment(\.scenePhase) private var scenePhase

    @State private var camera: MapCameraPosition = .region(MKCoordinateRegion(
        center: .init(latitude: 10.7769, longitude: 106.7009), // TP.HCM
        span: .init(latitudeDelta: 0.08, longitudeDelta: 0.08)))
    @State private var selected: CLLocationCoordinate2D?
    @State private var showSettings = false
    @State private var showFavorites = false
    @State private var showSearch = false
    @State private var showManual = false
    @State private var showSaveFavorite = false
    @State private var favoriteName = ""
    @State private var manualText = ""

    var body: some View {
        NavigationStack {
            MapReader { proxy in
                Map(position: $camera) {
                    if let selected {
                        Marker("Điểm đã chọn", systemImage: "mappin", coordinate: selected)
                            .tint(.orange)
                    }
                    if let spoofed = spoofer.spoofedCoordinate {
                        Annotation("Vị trí ảo", coordinate: spoofed) {
                            Image(systemName: "location.fill")
                                .font(.title3)
                                .foregroundStyle(.white)
                                .padding(8)
                                .background(.blue, in: Circle())
                                .accessibilityLabel("Vị trí ảo đang bật")
                        }
                    }
                }
                .mapControls {
                    MapCompass()
                    MapScaleView()
                }
                .onTapGesture { point in
                    if let coord = proxy.convert(point, from: .local) {
                        selected = coord
                    }
                }
            }
            .ignoresSafeArea(edges: .bottom)
            .safeAreaInset(edge: .bottom) { controlPanel }
            .navigationTitle("Vị Trí Ảo")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { showSettings = true } label: {
                        Label("Cài đặt", systemImage: "gearshape")
                    }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button { showSearch = true } label: { Label("Tìm", systemImage: "magnifyingglass") }
                    Button { showManual = true } label: { Label("Nhập toạ độ", systemImage: "number") }
                    Button { showFavorites = true } label: { Label("Yêu thích", systemImage: "star") }
                }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .sheet(isPresented: $showSearch) {
                SearchView { coord in
                    select(coord)
                }
                .presentationDetents([.medium, .large])
            }
            .sheet(isPresented: $showFavorites) {
                FavoritesView(store: favorites) { fav in
                    select(fav.coordinate)
                }
                .presentationDetents([.medium, .large])
            }
            .alert("Nhập toạ độ", isPresented: $showManual) {
                TextField("10.7769, 106.7009", text: $manualText)
                    .keyboardType(.numbersAndPunctuation)
                Button("Chọn") {
                    if let c = CoordinateParser.parse(manualText) {
                        select(c)
                    } else {
                        spoofer.lastError = "Không đọc được toạ độ: \(manualText)"
                    }
                    manualText = ""
                }
                Button("Huỷ", role: .cancel) { manualText = "" }
            } message: {
                Text("Dán \"vĩ độ, kinh độ\" hoặc link Google Maps.")
            }
            .alert("Lưu vào yêu thích", isPresented: $showSaveFavorite) {
                TextField("Tên", text: $favoriteName)
                Button("Lưu") {
                    if let selected {
                        favorites.add(name: favoriteName.isEmpty ? selected.display : favoriteName, coordinate: selected)
                    }
                    favoriteName = ""
                }
                Button("Huỷ", role: .cancel) { favoriteName = "" }
            }
            .onAppear {
                if !spoofer.hasPairingFile { showSettings = true }
            }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active {
                    spoofer.refreshPairingState()
                    spoofer.syncState()
                }
            }
        }
    }

    private func select(_ coord: CLLocationCoordinate2D) {
        selected = coord
        withAnimation {
            camera = .region(MKCoordinateRegion(center: coord, span: .init(latitudeDelta: 0.02, longitudeDelta: 0.02)))
        }
    }

    @ViewBuilder
    private var controlPanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Circle()
                    .fill(spoofer.isActive ? Color.green : Color.secondary)
                    .frame(width: 10, height: 10)
                Text(spoofer.isActive ? "Đang giả lập" : "Đang dùng GPS thật")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                if spoofer.isBusy { ProgressView() }
            }

            if let spoofed = spoofer.spoofedCoordinate {
                Text("Vị trí ảo: \(spoofed.display)")
                    .font(.footnote.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            if let selected {
                HStack {
                    Text(selected.display)
                        .font(.footnote.monospacedDigit())
                        .textSelection(.enabled)
                    Spacer()
                    Button {
                        showSaveFavorite = true
                    } label: {
                        Image(systemName: "star.badge.plus")
                    }
                    .accessibilityLabel("Lưu vào yêu thích")
                }
            } else {
                Text("Chạm lên bản đồ để chọn điểm.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            if let error = spoofer.lastError {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            HStack(spacing: 12) {
                Button {
                    if let selected { spoofer.start(at: selected) }
                } label: {
                    Label(spoofer.isActive ? "Dời tới đây" : "Bắt đầu", systemImage: "location.fill")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .disabled(selected == nil || spoofer.isBusy)

                Button(role: .destructive) {
                    spoofer.stop()
                } label: {
                    Label("Dừng", systemImage: "location.slash")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .disabled(!spoofer.isActive || spoofer.isBusy)
            }
            .controlSize(.large)
        }
        .padding()
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .padding(.horizontal)
        .padding(.bottom, 8)
    }
}

struct SearchView: View {
    var onPick: (CLLocationCoordinate2D) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var results: [MKMapItem] = []
    @State private var searching = false

    var body: some View {
        NavigationStack {
            List {
                if searching {
                    ProgressView()
                } else if results.isEmpty && !query.isEmpty {
                    ContentUnavailableView.search(text: query)
                }
                ForEach(results, id: \.self) { item in
                    Button {
                        onPick(item.placemark.coordinate)
                        dismiss()
                    } label: {
                        VStack(alignment: .leading) {
                            Text(item.name ?? "Không tên").foregroundStyle(.primary)
                            if let title = item.placemark.title {
                                Text(title).font(.footnote).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            .navigationTitle("Tìm địa điểm")
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Tên địa điểm, địa chỉ…")
            .onSubmit(of: .search) { Task { await search() } }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Đóng") { dismiss() } }
            }
        }
    }

    private func search() async {
        if let c = CoordinateParser.parse(query) {
            onPick(c)
            dismiss()
            return
        }
        searching = true
        defer { searching = false }
        let request = MKLocalSearch.Request()
        request.naturalLanguageQuery = query
        results = (try? await MKLocalSearch(request: request).start().mapItems) ?? []
    }
}

struct FavoritesView: View {
    @ObservedObject var store: FavoritesStore
    var onPick: (Favorite) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if store.items.isEmpty {
                    ContentUnavailableView("Chưa có địa điểm yêu thích",
                                           systemImage: "star",
                                           description: Text("Chọn một điểm trên bản đồ rồi bấm biểu tượng ngôi sao."))
                } else {
                    List {
                        ForEach(store.items) { fav in
                            Button {
                                onPick(fav)
                                dismiss()
                            } label: {
                                VStack(alignment: .leading) {
                                    Text(fav.name).foregroundStyle(.primary)
                                    Text(fav.coordinate.display)
                                        .font(.footnote.monospacedDigit())
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .onDelete(perform: store.remove)
                    }
                }
            }
            .navigationTitle("Yêu thích")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Đóng") { dismiss() } }
                if !store.items.isEmpty {
                    ToolbarItem(placement: .primaryAction) { EditButton() }
                }
            }
        }
    }
}
