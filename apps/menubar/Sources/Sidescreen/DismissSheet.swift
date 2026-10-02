import AppKit

/// The Dismissal question, laid over the Viewer's screen: what happens to
/// each window, then cancel or close. Esc cancels, ⌘Return confirms.
final class DismissSheet: NSView {
    /// The adjusted plan, or nil when the user cancels.
    var onAnswer: (([Dismissal.Item]?) -> Void)?

    private var items: [Dismissal.Item]
    private let box: NSView
    private var choices: [Int: NSSegmentedControl] = [:]
    private var confirm: ActionButton!
    private var live = false

    init(items: [Dismissal.Item], display: VirtualDisplay, live: Bool) {
        self.items = items
        let (surface, content) = Glass.make(cornerRadius: 16)
        box = surface
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.45).cgColor
        layer?.cornerRadius = 13
        layer?.cornerCurve = .continuous

        let title = NSTextField(labelWithString: "关闭「\(display.label)」？")
        title.font = .systemFont(ofSize: 15, weight: .semibold)
        var lines: [NSView] = [title]
        if live {
            lines.append(Self.note("\(display.agentName) 仍在使用这块显示器，关闭后它将无法继续操作。",
                                   color: .systemOrange))
        }
        lines.append(Self.note(items.isEmpty ? "这块显示器上没有窗口。" : "选择这些窗口的去向。",
                               color: .secondaryLabelColor))

        let stack = NSStackView(views: lines)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        if !items.isEmpty {
            stack.setCustomSpacing(14, after: lines.last!)
            stack.addArrangedSubview(rows())
        }
        let cancel = ActionButton(title: "取消") { [weak self] _ in self?.answer(false) }
        confirm = ActionButton(title: "关闭显示器") { [weak self] _ in self?.answer(true) }
        self.live = live
        let buttons = NSStackView(views: [NSView(), cancel, confirm])
        buttons.spacing = 8
        stack.addArrangedSubview(buttons)
        stack.setCustomSpacing(18, after: stack.arrangedSubviews[stack.arrangedSubviews.count - 2])
        buttons.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        box.translatesAutoresizingMaskIntoConstraints = false
        addSubview(box)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 18),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -16),
            box.widthAnchor.constraint(equalToConstant: 420),
            box.centerXAnchor.constraint(equalTo: centerXAnchor),
            box.centerYAnchor.constraint(equalTo: centerYAnchor),
            box.heightAnchor.constraint(lessThanOrEqualTo: heightAnchor, constant: -24),
        ])
        updateConfirm()
    }

    /// Red only when something is lost: a window closes, or the agent is
    /// cut off.
    @objc private func updateConfirm() {
        let closesWindow = choices.values.contains { $0.selectedSegment == 1 }
        confirm.setProminent(closesWindow || live ? .systemRed : .controlAccentColor)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private static func note(_ text: String, color: NSColor) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = .systemFont(ofSize: 12)
        label.textColor = color
        label.preferredMaxLayoutWidth = 380
        return label
    }

    /// One row per window, in a scroll view once there are many.
    private func rows() -> NSView {
        let list = NSStackView()
        list.orientation = .vertical
        list.alignment = .leading
        list.spacing = 10
        for (index, item) in items.enumerated() {
            let name = NSTextField(labelWithString: item.name)
            name.font = .systemFont(ofSize: 13, weight: .medium)
            name.lineBreakMode = .byTruncatingTail
            name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            let why = NSTextField(labelWithString: item.reason)
            why.font = .systemFont(ofSize: 11)
            why.textColor = .secondaryLabelColor
            let label = NSStackView(views: [name, why])
            label.orientation = .vertical
            label.alignment = .leading
            label.spacing = 1
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

            let fate: NSView
            if item.locked {
                // The user's own window always comes back; nothing to choose.
                let fixed = NSTextField(labelWithString: "取回原位置")
                fixed.font = .systemFont(ofSize: 12)
                fixed.textColor = .secondaryLabelColor
                fate = fixed
            } else {
                let choice = NSSegmentedControl(labels: ["取回", "关闭窗口"], trackingMode: .selectOne,
                                                target: self, action: #selector(updateConfirm))
                choice.controlSize = .small
                choice.selectedSegment = item.fate == .giveBack ? 0 : 1
                choices[index] = choice
                fate = choice
            }
            fate.setContentCompressionResistancePriority(.required, for: .horizontal)
            fate.setContentHuggingPriority(.required, for: .horizontal)
            let row = NSStackView(views: [label, NSView(), fate])
            row.spacing = 12
            row.alignment = .centerY
            list.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: list.widthAnchor).isActive = true
        }

        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        let document = FlippedView()
        document.addSubview(list)
        list.translatesAutoresizingMaskIntoConstraints = false
        document.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        let height = scroll.heightAnchor.constraint(equalTo: list.heightAnchor)
        height.priority = .defaultHigh
        NSLayoutConstraint.activate([
            list.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            list.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            list.topAnchor.constraint(equalTo: document.topAnchor),
            list.bottomAnchor.constraint(equalTo: document.bottomAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            scroll.widthAnchor.constraint(equalToConstant: 380),
            height,
            scroll.heightAnchor.constraint(lessThanOrEqualToConstant: 300),
        ])
        return scroll
    }

    func answer(_ confirmed: Bool) {
        guard let onAnswer else { return }
        self.onAnswer = nil
        guard confirmed else { return onAnswer(nil) }
        onAnswer(items.enumerated().map { index, item in
            var item = item
            if let choice = choices[index] { item.fate = choice.selectedSegment == 0 ? .giveBack : .close }
            return item
        })
    }

    /// Esc and ⌘Return answer; everything else stays here instead of
    /// reaching the display underneath.
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 53: answer(false)
        // Return alone could close windows by accident; ⌘Return confirms.
        case 36, 76 where event.modifierFlags.contains(.command): answer(true)
        default: break
        }
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func rightMouseDown(with event: NSEvent) {}
    override func scrollWheel(with event: NSEvent) {}
}

private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
