import AppKit
import SwiftUI

/// The library's search field: a native `NSSearchField` for the bar above the table.
/// Bumping `focusRequest` (Edit ▸ Find) focuses it.
struct LibrarySearchField: NSViewRepresentable {
    @Binding var text: String
    var focusRequest: Int

    func makeNSView(context: Context) -> NSSearchField {
        let field = SearchField()
        field.placeholderString = "Title, artist or album"
        field.sendsSearchStringImmediately = true
        field.delegate = context.coordinator
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        context.coordinator.text = $text
        if field.stringValue != text { field.stringValue = text }
        if focusRequest != context.coordinator.focusRequest {
            context.coordinator.focusRequest = focusRequest
            field.window?.makeFirstResponder(field)
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text, focusRequest: focusRequest)
    }

    /// Doesn't take focus when the window first opens; only on click or Find.
    final class SearchField: NSSearchField {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            DispatchQueue.main.async { [weak self] in
                guard let self, let window,
                      let editor = window.firstResponder as? NSText, editor.delegate === self
                else { return }
                window.makeFirstResponder(nil)
            }
        }
    }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var text: Binding<String>
        var focusRequest: Int

        init(text: Binding<String>, focusRequest: Int) {
            self.text = text
            self.focusRequest = focusRequest
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }
    }
}
