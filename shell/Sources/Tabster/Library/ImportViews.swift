import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Toolbar progress while an import runs.
struct ImportProgressView: View {
    let progress: ImportProgress

    var body: some View {
        HStack(spacing: 8) {
            ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1)))
                .frame(width: 100)
            Text("Importing \(progress.done) of \(progress.total)")
                .font(.callout)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 6)
    }
}

/// Lists the files the last import couldn't read.
struct ImportFailuresSheet: View {
    let failures: [ImportOutcome]
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(
                failures.count == 1 ? "1 file couldn't be imported" : "\(failures.count) files couldn't be imported",
                systemImage: "exclamationmark.triangle.fill"
            )
            .font(.headline)
            .symbolRenderingMode(.multicolor)
            List(failures, id: \.path) { failure in
                VStack(alignment: .leading, spacing: 2) {
                    Text(URL(filePath: failure.path).lastPathComponent)
                    Text(failure.error ?? "")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .help(failure.path)
            }
            .frame(minHeight: 120, maxHeight: 300)
            HStack {
                Spacer()
                Button("OK") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 460)
    }
}

enum ImportPanel {
    static let extensions = ["gp", "gpx", "gp5", "gp4", "gp3"]

    /// Asks for tab files and folders to import.
    @MainActor
    static func run() -> [URL] {
        let panel = NSOpenPanel()
        panel.title = "Import Tabs"
        panel.prompt = "Import"
        panel.message = "Choose Guitar Pro files, or folders to import every tab inside."
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.allowedContentTypes = extensions.compactMap { UTType(filenameExtension: $0) } + [.folder]
        return panel.runModal() == .OK ? panel.urls : []
    }
}
