# 06 · Mobile SDK

**Status:** Draft v0.1 · 2026-11-10

Mobile ships in two phases (`DECISIONS.md` D-04).

| Phase | What ships | Why |
|---|---|---|
| **A · WebView wrappers** | iOS, Android, React Native and Flutter packages that host the web renderer in a WebView | Parity with every competitor, at low cost, on day one |
| **B · Native renderer** | The Rust engine drawing through `wgpu` into a native view | The differentiator: no WebView, native gestures, faster start |

Both phases expose the **same public API** per platform, so a customer moves from A to B by changing a version number.

## 1. Public API (all platforms)

The surface mirrors `05-web-sdk.md`: the same option names, callbacks and methods, in each platform's idiom.

Swift:

```swift
let map = CaveaSeatMapView(options: .init(
    state: .caveaServer(baseURL: url, publishableKey: "pk_live_…", event: "hamlet-2027-03-14"),
    pricing: [.init(category: "vip", price: 9500)],
    selection: .init(max: 8, noOrphans: .lenient)
))
map.onSelectionChange = { selection in cart.update(selection) }
view.addSubview(map)
```

Kotlin:

```kotlin
val map = CaveaSeatMapView(context, CaveaOptions(
    state = CaveaState.server(baseUrl, publishableKey = "pk_live_…", event = "hamlet-2027-03-14"),
    pricing = listOf(CategoryPricing("vip", 9500)),
    selection = SelectionOptions(max = 8, noOrphans = OrphanPolicy.LENIENT),
))
map.onSelectionChange = { selection -> cart.update(selection) }
```

React Native and Flutter expose a component with the same props as the React wrapper, plus a controller for the imperative methods.

Custom state adapters are supported on every platform through a protocol or interface with `connect`, `hold`, `release`.

## 2. Phase A · WebView wrappers

### 2.1 Design

- A bundled HTML page loads the web renderer from assets **inside the app package**, so no network request is needed for the renderer itself.
- Options are passed as JSON at load. Callbacks cross the bridge as JSON messages. Methods are calls into the page.
- The chart and the state are fetched by the page as on the web.

| Platform | Host | Bridge |
|---|---|---|
| iOS | `WKWebView` | `WKScriptMessageHandler` |
| Android | `android.webkit.WebView` | `WebMessagePort` |
| React Native | The native views above, as a native component | Native events |
| Flutter | The native views above, as a platform view | Method channel |

React Native and Flutter wrap **our** native views, not a generic WebView package, so Phase B replaces the inside without touching the wrappers.

### 2.2 Requirements

- Renderer assets are versioned with the SDK; the page refuses to load assets of another version.
- The WebView has no navigation, no file access beyond the bundle, and no JavaScript interface other than the message bridge.
- Pinch and pan are handled by the renderer; the WebView's own zoom and scroll bounce are disabled.
- Safe-area insets, dark mode and Dynamic Type settings are passed in as theme tokens.
- VoiceOver and TalkBack use the web accessibility layer through the WebView.
- A fatal error in the page is reported through `onError`; the view shows a retry state.

### 2.3 Known limits of Phase A

- Start-up pays for WebView creation (typically hundreds of milliseconds on older phones).
- Memory is higher than native.
- Gesture feel is the browser's, not the platform's.

These are the same limits every competitor's mobile SDK has today.

## 3. Phase B · Native renderer

### 3.1 Architecture

```
Swift / Kotlin view  ──▶  UniFFI bindings  ──▶  cavea-mobile (Rust)
   │  touch, layout, display link                 │
   │  CoreText / Android text rasteriser          ├─ cavea-render engine
   │  accessibility elements                      ├─ wgpu backend → Metal / Vulkan / GLES
   ▼                                              └─ networking for the cavea-server adapter
CAMetalLayer / SurfaceView
```

| Concern | iOS | Android |
|---|---|---|
| Surface | `CAMetalLayer` in a `UIView` | `SurfaceView` → `ANativeWindow` |
| Frame clock | `CADisplayLink`, honouring ProMotion | `Choreographer` |
| Touch | Raw touches forwarded to the engine's gesture recogniser; coexists with parent scroll views through a gesture delegate | `onTouchEvent`, with `requestDisallowInterceptTouchEvent` while a gesture is active |
| Text | CoreText draws strings into bitmaps for the dynamic atlas | `android.graphics.Canvas` and `Paint` |
| Images | `ImageIO` | `ImageDecoder` |
| Networking | The adapter runs in Rust (`reqwest` with `rustls`, WebSocket), or the host supplies its own | Same |
| Lifecycle | Pause rendering and realtime in the background; resync on return | Same; recreate the surface on `surfaceCreated` |
| Threading | Engine on a dedicated render thread; callbacks delivered on the main thread | Same |

### 3.2 Bindings

- `cavea-mobile` defines the FFI surface in UniFFI: options, callbacks as a foreign trait, methods.
- Swift and Kotlin code is generated; a thin hand-written layer provides the idiomatic API of §1.
- Distribution: a Swift package with an XCFramework; an Android AAR on Maven with `arm64-v8a`, `armeabi-v7a` and `x86_64` libraries.

### 3.3 React Native and Flutter in Phase B

| Framework | Embedding |
|---|---|
| React Native | A Fabric native component hosting the native view |
| Flutter | A `Texture` widget backed by an external texture the engine renders into, with touches forwarded from Dart; a platform view as fallback |

### 3.4 Gestures

- The shared recogniser from `04-renderer.md` §14 is used so that behaviour matches the web.
- Platform-specific constants (deceleration rate, tap slop, double-tap interval) are read from the platform so that the map feels native.
- Haptic feedback on selection and on a refused hold, optional.

### 3.5 Accessibility

- The engine exposes an accessibility tree for the focused section: rows and seats with label, state and available actions.
- iOS: `UIAccessibilityElement` objects are created for the visible seats at detail level; the rotor navigates by row and section.
- Android: an `AccessibilityNodeProvider` supplies virtual nodes.
- The list view of `11-accessibility.md` §4 is provided as a native screen in both SDKs.

### 3.6 Budgets (targets)

| Metric | Target |
|---|---|
| Added app size, iOS (arm64, after App Store thinning) | ≤ 4 MB |
| Added app size, Android (per ABI) | ≤ 5 MB |
| Chart-ready, 20,000 seats, reference Android, warm network | ≤ 500 ms |
| Frame time, 95th percentile, 100,000 seats, reference Android | ≤ 16.6 ms |
| Memory, 100,000 seats | ≤ 40 MB |

### 3.7 Fallback

If the native surface cannot be created (very old devices, missing Vulkan and GLES 3), the SDK falls back to the Phase A WebView path automatically and reports which path is in use.

## 4. Minimum platform versions

| Platform | Phase A | Phase B |
|---|---|---|
| iOS | 15 | 15 |
| Android | API 24 | API 26 |
| React Native | 0.74, new architecture optional | New architecture |
| Flutter | 3.22 | 3.22 |

Confirm these against current usage figures when Phase A starts.

## 5. Testing

- Example apps for all four platforms, used for manual testing and screenshots.
- UI tests: load a chart, select two seats, verify the callback payload (XCUITest, Espresso, Detox, Flutter integration tests).
- Phase B: golden-image tests on device simulators against the web renderer's images, with a tolerance.
- Performance: the scripted gesture trace from `13-testing-benchmarks.md` §6 runs in the example apps and reports frame statistics.
- Accessibility: manual VoiceOver and TalkBack passes per release.
