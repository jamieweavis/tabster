import AppKit
import SwiftUI

/// Double-clicking a column divider in the table's header sizes the column to the left to fit
/// its header and contents, like Finder; the header's right-click menu gains "Size Column to Fit"
/// and "Size All Columns to Fit". Columns keep their widths when the list is resized. SwiftUI's `Table` has no hook for this, so it attaches
/// to the `NSTableView` behind it. Place as a background of the `Table`.
struct ColumnAutosizer: NSViewRepresentable {
    /// The widest content of the column with this header title, or nil to leave it alone.
    /// (SwiftUI's cells always fill their column, so they can't be measured directly.)
    var contentWidth: (String) -> CGFloat?

    func makeNSView(context: Context) -> Probe { Probe() }
    func updateNSView(_ view: Probe, context: Context) {
        view.contentWidth = contentWidth
        view.attachSoon()
    }

    final class Probe: NSView {
        private weak var table: NSTableView?
        var contentWidth: (String) -> CGFloat? = { _ in nil }
        private var monitor: Any?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if window == nil {
                if let monitor { NSEvent.removeMonitor(monitor) }
                monitor = nil
            }
            attachSoon()
        }

        /// The table may be built after this view; try on the next turn of the run loop.
        func attachSoon() {
            // Resizing the list mustn't stretch or squeeze the columns (SwiftUI may reset this).
            table?.columnAutoresizingStyle = .noColumnAutoresizing
            guard monitor == nil, window != nil else { return }
            DispatchQueue.main.async { [weak self] in self?.attach() }
        }

        private func attach() {
            guard monitor == nil, window != nil, let table = nearestTable() else { return }
            self.table = table
            table.columnAutoresizingStyle = .noColumnAutoresizing
            // A local monitor sees header clicks before the header does: a double-click on a
            // divider sizes that column; a right-click (or Control-click) shows the header menu
            // with the fit options added.
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
                let isMenuClick = event.type == .rightMouseDown
                    || (event.type == .leftMouseDown && event.modifierFlags.contains(.control))
                guard isMenuClick || event.clickCount == 2 else { return event }
                // Local monitors run on the main thread.
                nonisolated(unsafe) let event = event
                let handled = MainActor.assumeIsolated {
                    guard let self else { return false }
                    return isMenuClick ? self.showMenu(for: event) : self.sizeColumn(atDividerOf: event)
                }
                return handled ? nil : event
            }
        }

        /// The `NSTableView` closest to this view: searched from each ancestor outwards.
        private func nearestTable() -> NSTableView? {
            func find(_ view: NSView) -> NSTableView? {
                if let table = view as? NSTableView { return table }
                return view.subviews.lazy.compactMap(find).first
            }
            var view: NSView? = superview
            while let current = view {
                if let table = find(current) { return table }
                view = current.superview
            }
            return nil
        }

        /// Where `event` falls in the table's header, if it does.
        private func headerPoint(of event: NSEvent) -> (NSTableView, NSTableHeaderView, NSPoint)? {
            guard let table, let header = table.headerView, event.window === header.window else { return nil }
            let point = header.convert(event.locationInWindow, from: nil)
            return header.bounds.contains(point) ? (table, header, point) : nil
        }

        /// Sizes the column whose right-edge divider is under `event`; false if there isn't one.
        private func sizeColumn(atDividerOf event: NSEvent) -> Bool {
            guard let (table, header, point) = headerPoint(of: event) else { return false }
            let slop: CGFloat = 5
            guard let index = table.tableColumns.indices.first(where: { index in
                !table.tableColumns[index].isHidden && abs(header.headerRect(ofColumn: index).maxX - point.x) <= slop
            }) else { return false }
            sizeToFit(table.tableColumns[index])
            return true
        }

        /// Shows the header's own menu (the column show/hide items) plus the fit options.
        private func showMenu(for event: NSEvent) -> Bool {
            guard let (table, header, point) = headerPoint(of: event),
                  let menu = header.menu(for: event) else { return false }
            menu.delegate?.menuNeedsUpdate?(menu)
            menu.addItem(.separator())
            let index = header.column(at: point)
            if index >= 0, contentWidth(table.tableColumns[index].title) != nil {
                let item = NSMenuItem(title: "Size Column to Fit", action: #selector(sizeColumnToFit(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = table.tableColumns[index]
                menu.addItem(item)
            }
            let all = NSMenuItem(title: "Size All Columns to Fit", action: #selector(sizeAllColumnsToFit(_:)), keyEquivalent: "")
            all.target = self
            menu.addItem(all)
            NSMenu.popUpContextMenu(menu, with: event, for: header)
            return true
        }

        @objc private func sizeColumnToFit(_ sender: NSMenuItem) {
            if let column = sender.representedObject as? NSTableColumn { sizeToFit(column) }
        }

        @objc private func sizeAllColumnsToFit(_ sender: NSMenuItem) {
            for column in table?.tableColumns ?? [] where !column.isHidden { sizeToFit(column) }
        }

        /// Sizes `column` to its header and widest content; leaves fixed (icon) columns alone.
        private func sizeToFit(_ column: NSTableColumn) {
            guard let content = contentWidth(column.title) else { return }
            // The cell's padding: the gap between the header's text and its divider.
            let padding: CGFloat = 12
            let width = max(column.headerCell.cellSize.width, content + padding)
            column.width = min(max(ceil(width), column.minWidth), column.maxWidth)
        }
    }
}
