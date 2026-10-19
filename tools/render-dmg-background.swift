// 静态安装引导图；在 macOS 运行后生成用于 Finder 的 2× PNG。
import AppKit
import Foundation

let width: CGFloat = 660
let height: CGFloat = 400
let scale: CGFloat = 2
let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(width * scale), pixelsHigh: Int(height * scale),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
)!
bitmap.size = NSSize(width: width, height: height)
let context = NSGraphicsContext(bitmapImageRep: bitmap)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
// bitmap.size 已让 AppKit 使用 2× 缩放，不能再对绘图上下文重复缩放。

func color(_ hex: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 255) / 255,
            green: CGFloat((hex >> 8) & 255) / 255,
            blue: CGFloat(hex & 255) / 255, alpha: 1)
}

func text(_ value: String, top: CGFloat, size: CGFloat, weight: NSFont.Weight, tint: UInt32) {
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    let attributed = NSAttributedString(string: value, attributes: [
        .font: NSFont.systemFont(ofSize: size, weight: weight),
        .foregroundColor: color(tint), .paragraphStyle: style,
    ])
    attributed.draw(in: NSRect(x: 32, y: height - top - 32, width: width - 64, height: 32))
}

color(0xf4f1e8).setFill()
NSBezierPath(rect: NSRect(x: 0, y: 0, width: width, height: height)).fill()
text("Install ClearFrame Desktop", top: 24, size: 23, weight: .semibold, tint: 0x111411)
text("Drag the app into Applications", top: 64, size: 14, weight: .medium, tint: 0x111411)
text("将应用拖入「应用程序」文件夹", top: 88, size: 12, weight: .regular, tint: 0x62665e)
text("アプリを「アプリケーション」フォルダへドラッグ", top: 108, size: 12, weight: .regular, tint: 0x62665e)

// 图标由 Finder 显示；箭头只占两者之间的空白，不叠加假图标。
let arrow = NSBezierPath()
arrow.move(to: NSPoint(x: 286, y: height - 190))
arrow.line(to: NSPoint(x: 373, y: height - 190))
arrow.move(to: NSPoint(x: 355, y: height - 172))
arrow.line(to: NSPoint(x: 373, y: height - 190))
arrow.line(to: NSPoint(x: 355, y: height - 208))
arrow.lineWidth = 6
arrow.lineCapStyle = .round
arrow.lineJoinStyle = .round
color(0x27835d).setStroke()
arrow.stroke()

let divider = NSBezierPath()
divider.move(to: NSPoint(x: 60, y: height - 290))
divider.line(to: NSPoint(x: 600, y: height - 290))
divider.lineWidth = 1
color(0xc9cdc3).setStroke()
divider.stroke()
text("After copying, open ClearFrame Desktop from Applications", top: 304, size: 12, weight: .medium, tint: 0x111411)
text("复制完成后，从「应用程序」打开 ClearFrame Desktop", top: 326, size: 11, weight: .regular, tint: 0x62665e)
text("コピー後、「アプリケーション」から ClearFrame Desktop を起動", top: 346, size: 11, weight: .regular, tint: 0x62665e)
NSGraphicsContext.restoreGraphicsState()

let output = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    .appendingPathComponent("src-tauri/installer/background.png")
try FileManager.default.createDirectory(at: output.deletingLastPathComponent(), withIntermediateDirectories: true)
try bitmap.representation(using: .png, properties: [:])!.write(to: output, options: .atomic)
print("Generated \(output.path) (1320×800 pixels, 660×400 points)")
