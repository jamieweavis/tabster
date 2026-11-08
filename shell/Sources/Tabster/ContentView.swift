import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            Text("Tabster").font(.largeTitle)
            if let core = model.core {
                Text("Core \(core.version())").foregroundStyle(.secondary)
            } else {
                Text(model.startupError ?? "Core failed to start").foregroundStyle(.red)
            }
        }
        .frame(minWidth: 600, minHeight: 400)
    }
}
