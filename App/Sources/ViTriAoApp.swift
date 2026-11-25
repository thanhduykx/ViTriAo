import SwiftUI

@main
struct ViTriAoApp: App {
    @StateObject private var spoofer = LocationSpoofer()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(spoofer)
        }
    }
}
