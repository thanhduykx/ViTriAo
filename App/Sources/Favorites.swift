import Foundation
import SwiftUI
import CoreLocation

struct Favorite: Codable, Identifiable, Hashable {
    var id = UUID()
    var name: String
    var latitude: Double
    var longitude: Double

    var coordinate: CLLocationCoordinate2D { .init(latitude: latitude, longitude: longitude) }
}

@MainActor
final class FavoritesStore: ObservableObject {
    @Published private(set) var items: [Favorite] = []
    private let key = "favorites.v1"

    init() {
        if let data = UserDefaults.standard.data(forKey: key),
           let decoded = try? JSONDecoder().decode([Favorite].self, from: data) {
            items = decoded
        }
    }

    func add(name: String, coordinate: CLLocationCoordinate2D) {
        items.insert(Favorite(name: name, latitude: coordinate.latitude, longitude: coordinate.longitude), at: 0)
        save()
    }

    func remove(at offsets: IndexSet) {
        items.remove(atOffsets: offsets)
        save()
    }

    private func save() {
        if let data = try? JSONEncoder().encode(items) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }
}

enum CoordinateParser {
    /// Nhận "10.7769, 106.7009", "10.7769 106.7009" hoặc link Google Maps có "@lat,lon" / "q=lat,lon".
    static func parse(_ text: String) -> CLLocationCoordinate2D? {
        let pattern = #"(-?\d{1,2}(?:\.\d+)?)\s*[,; ]\s*(-?\d{1,3}(?:\.\d+)?)"#
        guard let regex = try? NSRegularExpression(pattern: pattern),
              let m = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
              let r1 = Range(m.range(at: 1), in: text),
              let r2 = Range(m.range(at: 2), in: text),
              let lat = Double(text[r1]), let lon = Double(text[r2]),
              (-90...90).contains(lat), (-180...180).contains(lon)
        else { return nil }
        return .init(latitude: lat, longitude: lon)
    }
}

extension CLLocationCoordinate2D {
    var display: String { String(format: "%.6f, %.6f", latitude, longitude) }
}
