// Can this runner host a GPU window at all?
//
// A control for the macOS run smoke, and deliberately sharing nothing with
// sindon but the question. Linux taught this the expensive way: the first
// attempt to run there was under WSLg, where the compositor itself segfaulted,
// and the failure looked exactly like a sindon bug for two hours. What settled
// it was a control that drew — a winit-only version stayed alive and pointed
// the finger the wrong way, because a client that never attaches a buffer
// never asks the compositor to do the thing that was broken.
//
// So this presents real frames through Metal, in a real window, before the
// smoke runs. Swift because Xcode is preinstalled: no crates, no shared
// dependency graph, seconds to compile. If this fails, the runner has no
// window server or no GPU and the smoke's result says nothing about sindon.

import AppKit
import Metal
import QuartzCore

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

guard let device = MTLCreateSystemDefaultDevice() else {
    fail("control: no Metal device on this runner")
}
print("control: Metal device = \(device.name)")

guard let queue = device.makeCommandQueue() else {
    fail("control: could not create a Metal command queue")
}

// .accessory keeps a bare binary (no .app bundle) out of the Dock while still
// giving it a connection to the window server — the same situation a `cargo
// run` of an example is in.
NSApplication.shared.setActivationPolicy(.accessory)

let size = CGSize(width: 320, height: 240)
let window = NSWindow(
    contentRect: NSRect(origin: .zero, size: size),
    styleMask: [.titled],
    backing: .buffered,
    defer: false
)
guard let content = window.contentView else {
    fail("control: window has no content view")
}

// Layer-hosting view: assign the layer first, then set wantsLayer. The other
// order gives AppKit ownership and quietly replaces this layer.
let layer = CAMetalLayer()
layer.device = device
layer.pixelFormat = .bgra8Unorm
layer.framebufferOnly = true
layer.drawableSize = size
layer.frame = content.bounds
content.layer = layer
content.wantsLayer = true

window.makeKeyAndOrderFront(nil)
print("control: window ordered front")

var presented = 0
for frame in 0..<10 {
    guard let drawable = layer.nextDrawable() else {
        print("control: frame \(frame): no drawable")
        continue
    }
    let pass = MTLRenderPassDescriptor()
    pass.colorAttachments[0].texture = drawable.texture
    pass.colorAttachments[0].loadAction = .clear
    pass.colorAttachments[0].storeAction = .store
    pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.1, green: 0.2, blue: 0.3, alpha: 1.0)

    guard let buffer = queue.makeCommandBuffer(),
        let encoder = buffer.makeRenderCommandEncoder(descriptor: pass)
    else {
        fail("control: could not encode a render pass")
    }
    encoder.endEncoding()
    buffer.present(drawable)
    buffer.commit()
    buffer.waitUntilCompleted()
    if let error = buffer.error {
        fail("control: GPU error presenting frame \(frame): \(error)")
    }
    presented += 1
}

if presented == 0 {
    fail("control: Metal never produced a drawable — nothing can present here")
}
print("control: presented \(presented) frame(s) — this runner can host a GPU window")
