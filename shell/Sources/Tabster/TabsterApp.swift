import SwiftUI

@main
struct TabsterApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup("Tabster") {
            ContentView()
                .environment(model)
        }
    }
}
