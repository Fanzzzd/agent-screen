# Efficient UI recordings on macOS

## Recommendation

Prefer HEVC in MP4 for local AVPlayer replay when ScreenCaptureKit reports it available. Keep H.264 as an explicit capability fallback. Keep the existing capture, separate audio stream and passthrough join for the first change. Do not rewrite the pipeline before measuring the codec change.

The intended codec is hardware HEVC. **SCRecordingOutput cannot require or report hardware encoding through its documented public configuration.** Report its hardware status as unverified. If hardware-only encoding is a release requirement, the small codec change cannot satisfy it. Use an owned encoder pipeline instead. [1][4]

HEVC is a reasonable candidate because this M3 Max has HEVC media hardware and Apple supports HEVC in MP4 through AVFoundation. Neither fact proves a particular ScreenCaptureKit recording uses that hardware or saves a particular amount of space. Claim no compression percentage, CPU reduction or encoding-speed improvement before measurement. [3][5]

## Evidence and API limits

Reviewed on 2026-10-27. Sources are Apple documentation, WWDC24 session 10088, the installed Apple macOS 26.2 SDK headers, and project source at `origin/main`, commit `a1fe8218c64fde3786e337fd003f5b16a4fc0288`. Apple's live documentation is mutable. API availability below refers to macOS.

- **Recording output.** `SCRecordingOutputConfiguration` starts at macOS 15.0. The 26.2 SDK declares output URL, video codec, file type and their available-value arrays. It defaults to H.264 and MPEG-4. It exposes no bitrate, compression quality, audio codec, hardware requirement or selected-encoder property. Live docs also list audio/microphone mixing, which does not add encoder controls. [1]
- **HEVC and MP4.** Apple's WWDC24 example selects `.hevc` with `.mov`. Apple separately documents HEVC in MPEG-4 and local MPEG-4 playback through AVPlayer. These establish format support, not a ScreenCaptureKit codec/container compatibility matrix. Query `availableVideoCodecTypes` and `availableOutputFileTypes`, then handle output creation/start failures. Do not treat the two independent lists as proof that every combination works. [1][2][5]
- **Hardware requirement.** `kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder`, available since macOS 10.9, makes `VTCompressionSessionCreate` fail when hardware cannot satisfy the request. Reasons include unsupported configuration and busy hardware resources. `EnableHardwareAcceleratedVideoEncoder` only permits hardware. Query `kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder` on the actual owned session to confirm selection. An encoder-list entry proves capability, not which encoder ScreenCaptureKit chose. [4]
- **Rate and quality.** VideoToolbox's `AverageBitRate`, available since macOS 10.8, is a desired long-term rate, not a hard cap. Its `Quality` property is encoder-dependent. Equal numeric quality values do not establish equal text readability across codecs. Query supported properties and check every setter result. These controls do not become ScreenCaptureKit configuration options. [6]
- **Frame timing.** `minimumFrameInterval`, available since macOS 12.3, throttles updates. An explicit `1/30` requests at most 30 updates per second. The live documentation says the default is zero; the 26.2 SDK says `1/60`. Keep an explicit value. `SCFrameStatus.idle` means no new frame was generated because the display did not change. This does not document how SCRecordingOutput writes static periods, duplicate samples or bitrate. Do not promise adaptive file-size savings. [7]
- **M3 Max.** Apple's model specifications list hardware-accelerated H.264/HEVC, two video encode engines and AV1 decode. Apple's M3 announcement also identifies AV1 specifically as decoding. Neither advertises AV1 encoding. The parent's supplied local encoder-list result contains hardware HEVC/H.264/ProRes and no AV1 encoder. Do not add AV1 or repeat the machine probe. ProRes offers no established disk-efficiency benefit for this task. [3]

## Implementation scope

The inspected `apps/macos/src/mac/recording.rs` writes native-pixel BGRA video, explicitly caps updates at 30 fps and hides the captured cursor. It currently selects H.264/MP4. The separate application-filtered sound stream uses a 64×64 H.264/MOV picture at 2 fps, 48 kHz stereo audio and excludes this process's audio. `apps/macos/src/mac/mux.rs` copies only the main video track and the sound track into a composition, aligns their start offsets and exports MPEG-4 with `AVAssetExportPresetPassthrough`. The temporary sound picture never belongs in the final MP4. [10]

For the first implementation:

1. Query runtime recording codecs and file types. Prefer HEVC/MP4 when advertised. Use H.264/MP4 when HEVC is absent. If neither works, return an actionable error. Extend the Rust binding if necessary rather than hardcoding availability from the Mac model.
2. Log the offered codecs, requested codec, chosen codec/container and fallback reason. Store the chosen codec and hardware status in recording metadata. With SCRecordingOutput, use `hardware=unverified`; do not label it hardware-confirmed from the encoder list.
3. A codec-specific initialization/start failure may justify one clean H.264 retry before recording begins. Preserve the original error. Permission, disk and unrelated capture errors do not justify codec fallback. Do not restart a running recording silently or discard elapsed content.
4. Keep resolution, frame interval, audio filtering and pointer handling unchanged for the codec comparison. H.264 remains a compatibility fallback, not the automatic choice for local AVPlayer replay. Verify actual output codec, playback, seeking and the audio join before accepting HEVC as the default.

Keep passthrough export. Apple defines it as exporting in the current format; the SDK specifies passing media exactly as stored, subject to container restrictions. Check the export session's supported file types and final status. Do not substitute a highest-quality export preset, which would introduce another encode. Preserve source files until a joined output succeeds. Container rewriting and temporary duplicate files still cost I/O and peak disk space. [8]

Use AAC for compressed audio in a custom writer. Do not expand the final track into PCM or repeatedly re-encode AAC. The reviewed ScreenCaptureKit sources do not specify its audio codec or expose an audio-codec selector. Inspect the resulting audio format description before describing the existing path as AAC. A video-only policy change should pass its existing compressed audio through unchanged. If it is not AAC, handle audio-only conversion as a separate decision without re-encoding video. Apple documents HEVC/AAC output in AVFoundation. [1][9]

Keep the Agent Pointer and action timeline in the existing sidecar. Replay should draw the pointer over AVPlayer output. Burning it into video would change every pointer-motion frame and make the overlay permanent. This is a design reason to preserve the sidecar, not a measured compression claim. [10]

## When to replace SCRecordingOutput

A custom `SCStreamOutput` → encoder → `AVAssetWriter` path buys explicit encoder controls but adds timestamp handling, backpressure, buffer lifetime, final-frame duration and failure cleanup. Preserve the application-filtered audio boundary during any rewrite.

Two options exist:

- **AVAssetWriter-managed compression.** On macOS 10.10+, `AVVideoEncoderSpecificationKey` accepts VideoToolbox encoder-specification keys, including a hardware requirement. This can avoid owning a separate VT session when writer-level settings suffice. It does not expose an owned VT session for the active-hardware query. [11]
- **Owned VTCompressionSession.** Require hardware, query the selected encoder, set supported rate/quality controls, then feed compressed samples to AVAssetWriter. Set writer output settings to `nil` to avoid a second encode. For MP4 passthrough, Apple requires a nonnull source format hint. Also handle encoder draining and decode/presentation timing when frames reorder. [4][6][12]

Choose this work only if hardware enforcement is mandatory, the default recording quality fails text checks, or measured disk/CPU results justify explicit rate control. Do not promise zero-copy. Pixel conversion, encoder requirements and actual buffer handling need evidence.

## Bounded synthetic capability check

Ran once per codec on this M3 Max, macOS 26.2 build 25C56, with the Apple 26.2 SDK. An independent Swift helper generated owned UI-like frames offscreen with Core Graphics and Core Text. It used no ScreenCaptureKit, NSApplication, windows, displays, capture, audio, input taps or user recordings. H.264 ran first, then HEVC. [13]

Both runs used 1280×800 BGRA frames, 300 frames at 30 fps, and 10 seconds of media. The sequence contained text, scrolling, typing, menu changes, light/dark backgrounds and a final static period. Each owned VTCompressionSession set `RequireHardwareAcceleratedVideoEncoder=true`, prepared the encoder and explicitly queried `UsingHardwareAcceleratedVideoEncoder`. AVAssetWriter only passed compressed samples into MP4, with `outputSettings=nil` and a format hint. [4][12][13]

| Codec | Success | Hardware query | Samples written | MP4 bytes | Wall seconds | Process CPU seconds |
| --- | --- | --- | --- | --- | --- | --- |
| H.264 | Yes | true | 300 | 3,525,043 | 0.736 | 0.442 |
| HEVC | Yes | true | 300 | 2,873,531 | 0.550 | 0.374 |

All property setters, the hardware query, encoder preparation, frame submission and encoder draining returned success. Writer start, append and finish succeeded. No software fallback path exists. Drawing took 0.265 seconds for H.264 and 0.255 seconds for HEVC. Wall and CPU times include generation, session setup, encoding and file finalization. They are single-run capability observations, not isolated encoder throughput measurements. [13]

The helper requested the same 3,000,000-bit/s average target, expected frame rate of 30, maximum keyframe duration of 2 seconds, no frame reordering and non-real-time encoding for both codecs. That bitrate is a soft target. No decoded readability or equal-quality comparison was performed. The smaller HEVC fixture does not establish a same-quality compression advantage, a CPU advantage or any SCRecordingOutput result. No AVPlayer playback test had run at the time of this encoding experiment. Subsequent playback checks are recorded below. [6][13]

The helper allowed at most four submitted frames in flight and drained its callback queue during submission. It did not retain all source frames. An 85-second watchdog and a 90-second parent timeout bounded execution. Each output had a 49,000,000-byte OS file-size limit, below 100 MB combined. Actual media output totaled 6,398,574 bytes. No retry or benchmark sweep ran. [13]

The two silent MP4 fixtures remain outside the repository for later headless playback tests:

- `/Users/fanzhende/.asq/recording-media.wleWMv/synthetic-h264-1280x800-30fps.mp4`
- `/Users/fanzhende/.asq/recording-media.wleWMv/synthetic-hevc-1280x800-30fps.mp4`

## Subsequent headless playback checks

The playback implementation's `apps/macos/tests/PlaybackProbe.m` compiles
against the production `apps/macos/native/Playback.m` bridge. It creates no
NSApplication, window, display or capture. Both owned 10-second fixtures
above passed native AVPlayer readiness, 1280×800 decode, a paused poster,
play, pause, forward/backward and rapid seeks. The pause media-time delta
was zero in both runs. The probe also checks a local 420f pixel-buffer format
that the GPUI surface path accepts. These are backend checks, not a visual
GPUI acceptance test.

The fixtures contain no audio. Audio continuity across overlay toggles,
ScreenCaptureKit output codec/encoder selection and equal-quality
compression remain unverified. A silent decoder test cannot establish any
of those results. The app's pointer toggle changes only overlay state; it
does not call AVPlayer play, pause or seek.

## Full synthetic benchmark plan, not executed

Run no capture APIs, create no displays and request no input or audio taps. Generate deterministic UI-like pixel buffers offscreen with Core Graphics/Core Text. Encode one candidate at a time.

1. Generate 120-second SDR sequences at the same native pixel dimensions. Include light/dark text, small colored syntax, thin lines, scrolling, typing, menu transitions and long static periods. Retain source frames or a deterministic generator for pixel-aligned comparisons.
2. Encode hardware-required H.264 and HEVC with identical frame timestamps, dimensions and pixel format. Hold the keyframe/reordering policy fixed where both support it. Start at 30 fps. Test a separate 15 fps variant and an explicitly sparse-timestamp variant later. Sparse input tests our writer's timing; it does not establish ScreenCaptureKit's static-frame behavior.
3. Sweep supported bitrate or quality settings. Match decoded text readability first. Inspect small glyphs, colored text edges, scrolling frames and the first settled frame after motion at 1:1 pixels. Record OCR errors against known text and pixel-error metrics as supporting checks. Reject visible character damage even when a whole-frame score looks acceptable.
4. Record video bytes, total MP4 bytes, duration and achieved bitrate. Measure wall time, process user/system CPU time, peak memory, encoded frames per second and dropped/error samples. Separate generation time from encoding time. Include encoder callbacks and drain time. Use one warm-up and three measured runs per candidate in alternating order.
5. Run maximum-throughput encoding and separately paced 30 fps feeding. Report throughput relative to 30 fps and CPU seconds per media second. Record OS/SDK, codec/profile, pixel format, accepted properties, actual hardware status and sample count. Test compressed AAC with generated silence and tones; measure audio and temporary-file bytes separately.
6. Validate outputs headlessly with AVAssetReader and AVPlayer item readiness/seeks. Compare compressed video samples before and after the audio join to check that muxing preserved them. Schedule visual in-app playback and integrated ScreenCaptureKit checks separately when permitted.

This benchmark compares available encoders and a possible writer pipeline. SCRecordingOutput has no documented generated-frame input, so it cannot prove that a minimal ScreenCaptureKit codec change produces the same size, quality or CPU results. Ship no numerical saving claim from the synthetic result alone.

## Sources

1. Apple, [SCRecordingOutputConfiguration](https://developer.apple.com/documentation/screencapturekit/screcordingoutputconfiguration), [availableVideoCodecTypes](https://developer.apple.com/documentation/screencapturekit/screcordingoutputconfiguration/availablevideocodectypes), [availableOutputFileTypes](https://developer.apple.com/documentation/screencapturekit/screcordingoutputconfiguration/availableoutputfiletypes). macOS 15.0+. Defaults and configuration surface cross-checked against Apple's macOS 26.2 SDK `ScreenCaptureKit.framework/Headers/SCRecordingOutput.h`.
2. Apple, [Capture HDR content with ScreenCaptureKit, WWDC24 session 10088, Record to file](https://developer.apple.com/videos/play/wwdc2024/10088/?time=578). HEVC/MOV example and recording lifecycle.
3. Apple, [MacBook Pro with M3 Pro or M3 Max, November 2023, technical specifications](https://support.apple.com/en-us/117736), [M3 family announcement](https://www.apple.com/newsroom/2023/10/apple-unveils-m3-m3-pro-and-m3-max-the-most-advanced-chips-for-a-personal-computer/). Model-specific media engines and AV1 decoding.
4. Apple, [RequireHardwareAcceleratedVideoEncoder](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_requirehardwareacceleratedvideoencoder), [EnableHardwareAcceleratedVideoEncoder](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablehardwareacceleratedvideoencoder), [UsingHardwareAcceleratedVideoEncoder](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_usinghardwareacceleratedvideoencoder). macOS 10.9+. Cross-checked against 26.2 SDK `VideoToolbox.framework/Headers/VTCompressionProperties.h`.
5. Apple, [Video technology overview](https://developer.apple.com/documentation/technologyoverviews/video), sections Prepare video assets for playback and Play video content. HEVC containers and AVPlayer.
6. Apple, [AverageBitRate](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_averagebitrate), [Quality](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_quality), [VTSessionCopySupportedPropertyDictionary](https://developer.apple.com/documentation/videotoolbox/vtsessioncopysupportedpropertydictionary(_:supportedpropertydictionaryout:)). macOS 10.8+.
7. Apple, [minimumFrameInterval](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/minimumframeinterval), [SCFrameStatus.idle](https://developer.apple.com/documentation/screencapturekit/scframestatus/idle). macOS 12.3+. Default discrepancy checked against 26.2 SDK `ScreenCaptureKit.framework/Headers/SCStream.h`.
8. Apple, [AVAssetExportPresetPassthrough](https://developer.apple.com/documentation/avfoundation/avassetexportpresetpassthrough), [AVAssetExportSession.supportedFileTypes](https://developer.apple.com/documentation/avfoundation/avassetexportsession/supportedfiletypes). macOS 10.7+. Exact-media semantics cross-checked against 26.2 SDK `AVFoundation.framework/Headers/AVAssetExportSession.h`.
9. Apple, [AVAssetExportPresetHEVCHighestQuality](https://developer.apple.com/documentation/avfoundation/avassetexportpresethevchighestquality). macOS 10.13+. Evidence for HEVC/AAC support, not a recommendation to transcode the recording.
10. Project primary source, [`recording.rs`](https://github.com/Fanzzzd/agent-screen/blob/a1fe8218c64fde3786e337fd003f5b16a4fc0288/apps/macos/src/mac/recording.rs), [`mux.rs`](https://github.com/Fanzzzd/agent-screen/blob/a1fe8218c64fde3786e337fd003f5b16a4fc0288/apps/macos/src/mac/mux.rs), and local `CONTEXT.md` definitions of Recording and Agent Pointer. Pointer sidecar replay is a recommendation, not a claim about an implemented player.
11. Apple, [AVVideoEncoderSpecificationKey](https://developer.apple.com/documentation/avfoundation/avvideoencoderspecificationkey). macOS 10.10+.
12. Apple, [AVAssetWriterInput.init(mediaType:outputSettings:)](https://developer.apple.com/documentation/avfoundation/avassetwriterinput/init(mediatype:outputsettings:)), macOS 10.7+, and [initializer with sourceFormatHint](https://developer.apple.com/documentation/avfoundation/avassetwriterinput/init(mediatype:outputsettings:sourceformathint:)), macOS 10.8+.
13. Local primary experiment, 2026-10-27. Helper `/Users/fanzhende/.asq/recording-helper.8XA2Po/encode_probe.swift`; raw results `/Users/fanzhende/.asq/recording-helper.8XA2Po/result.txt`. Compiled with the installed Apple Swift compiler using `-O`. No downloaded code or dependencies. The compiler emitted a Swift warning about the `CFTypeRef` out-parameter bridge in the hardware-property query. The query returned success and true; the independent require-hardware session constraint also succeeded.
