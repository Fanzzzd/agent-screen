import AppKit

/// What the user reads about a display's state. Peek and the Viewer show
/// the same words.
@MainActor
struct DisplayStatus {
    enum Kind { case manual, agent, help, takeover, review, disconnected }

    let kind: Kind
    /// Short form for Peek: "需要帮助：输入验证码".
    let short: String
    /// Long form for the Viewer header.
    let long: String
    let symbol: String?
    let color: NSColor

    init(_ display: VirtualDisplay, windowCount: Int) {
        let agent = display.agentName
        let windows = "\(windowCount) 个窗口"
        if display.ended {
            if display.endReason == .disconnected {
                kind = .disconnected
                short = "连接中断 · \(windows)"
                long = "\(agent) 的连接中断了，窗口还在这里"
                symbol = "exclamationmark.circle"
            } else {
                kind = .review
                short = "待查看 · \(windows)"
                long = "\(agent) 已停止操作，窗口等你查看"
                symbol = "tray"
            }
            color = .secondaryLabelColor
        } else if display.takeover {
            kind = .takeover
            short = "你在操作"
            long = "你在操作，\(agent) 在等你交还"
            symbol = nil
            color = .controlAccentColor
        } else if let reason = display.helpRequest {
            kind = .help
            short = "需要帮助：\(reason)"
            long = "需要你帮助：\(reason)"
            symbol = "hand.raised.fill"
            color = .systemOrange
        } else if display.owner == nil {
            kind = .manual
            short = "你新建的"
            long = "你新建的显示器，没有 agent 在用"
            symbol = nil
            color = .secondaryLabelColor
        } else {
            kind = .agent
            short = "由 \(agent) 控制"
            long = "由 \(agent) 控制"
            symbol = nil
            color = .secondaryLabelColor
        }
    }

    /// Status text with its symbol in front, for a label.
    func attributed(_ text: String, size: CGFloat, alignment: NSTextAlignment = .natural) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let font = NSFont.systemFont(ofSize: size)
        if let symbol, let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: size, weight: .medium)) {
            let attachment = NSTextAttachment()
            attachment.image = image
            let mark = NSMutableAttributedString(attachment: attachment)
            mark.addAttribute(.foregroundColor, value: color, range: NSRange(location: 0, length: mark.length))
            result.append(mark)
            result.append(NSAttributedString(string: " "))
        }
        result.append(NSAttributedString(string: text, attributes: [.font: font]))
        result.addAttribute(.foregroundColor, value: color, range: NSRange(location: 0, length: result.length))
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = alignment
        paragraph.lineBreakMode = .byTruncatingTail
        result.addAttribute(.paragraphStyle, value: paragraph, range: NSRange(location: 0, length: result.length))
        return result
    }
}
