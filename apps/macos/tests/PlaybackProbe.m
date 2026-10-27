// Headless probe of the same AVPlayer bridge used by the GPUI window.
// Input must be the caller's synthetic, silent MP4. No NSApplication.
#import <Foundation/Foundation.h>
#import <CoreVideo/CoreVideo.h>
#import <math.h>
#import <stdlib.h>
extern void *ASPlaybackCreate(const char *path);
extern void ASPlaybackDestroy(void *);
extern void ASPlaybackSetPlaying(void *, bool);
extern void ASPlaybackSeek(void *, double);
extern CVPixelBufferRef ASPlaybackSnapshot(void *, double *, double *, int *, bool *, bool *);
static bool lastPlaying = false, lastSeeking = false, badFrame = false;
static size_t width = 0, height = 0;

static double sample(void *player, int *frames, double *duration, int *status) {
    double time = 0;
    CVPixelBufferRef frame = ASPlaybackSnapshot(player, &time, duration, status, &lastPlaying, &lastSeeking);
    if (frame) {
        *frames += 1;
        width = CVPixelBufferGetWidth(frame); height = CVPixelBufferGetHeight(frame);
        badFrame |= CVPixelBufferGetPixelFormatType(frame) != kCVPixelFormatType_420YpCbCr8BiPlanarFullRange || CVPixelBufferGetPlaneCount(frame) != 2 || CVPixelBufferGetIOSurface(frame) == NULL;
        CVPixelBufferRelease(frame);
    }
    return time;
}

static double pump(void *player, double length, int *frames, double *duration, int *status) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:length];
    double time = 0;
    do {
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        time = sample(player, frames, duration, status);
    } while ([deadline timeIntervalSinceNow] > 0);
    return time;
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 2 && argc != 3) return 2;
        double expectedDuration = argc == 3 ? strtod(argv[2], NULL) : 2.0;
        if (!isfinite(expectedDuration) || expectedDuration < 1.8) return 2;
        void *player = ASPlaybackCreate(argv[1]);
        if (!player) return 3;
        int frames = 0, status = 0;
        double duration = 0;
        pump(player, 1.0, &frames, &duration, &status);
        int posterFrames = frames;
        ASPlaybackSetPlaying(player, true);
        double running = pump(player, 0.4, &frames, &duration, &status);
        ASPlaybackSetPlaying(player, false);
        double paused = pump(player, 0.2, &frames, &duration, &status);
        double still = pump(player, 0.2, &frames, &duration, &status);
        ASPlaybackSeek(player, 1.2);
        int beforeSeek = frames;
        double forward = pump(player, 0.7, &frames, &duration, &status);
        int forwardFrames = frames - beforeSeek;
        ASPlaybackSeek(player, 0.2);
        beforeSeek = frames;
        double backward = pump(player, 0.7, &frames, &duration, &status);
        int backwardFrames = frames - beforeSeek;
        ASPlaybackSeek(player, 1.6); ASPlaybackSeek(player, 0.4); ASPlaybackSeek(player, 1.0);
        beforeSeek = frames;
        double rapid = pump(player, 0.7, &frames, &duration, &status);
        int rapidFrames = frames - beforeSeek;
        // A drag across the bar: a seek every 15 ms. Frames must keep
        // arriving during the drag, then land exactly on release.
        beforeSeek = frames;
        for (int i = 0; i < 20; i++) {
            ASPlaybackSeek(player, 0.2 + i * 0.07);
            pump(player, 0.015, &frames, &duration, &status);
        }
        int dragFrames = frames - beforeSeek;
        ASPlaybackSeek(player, 1.4);
        beforeSeek = frames;
        double settled = pump(player, 0.7, &frames, &duration, &status);
        int settledFrames = frames - beforeSeek;
        bool settledSeeking = lastSeeking;
        ASPlaybackSeek(player, expectedDuration);
        beforeSeek = frames;
        double end = pump(player, 0.7, &frames, &duration, &status);
        int endFrames = frames - beforeSeek;
        ASPlaybackSeek(player, 0.0); ASPlaybackSetPlaying(player, true);
        double replay = pump(player, 0.25, &frames, &duration, &status);
        ASPlaybackSetPlaying(player, false);
        pump(player, 0.05, &frames, &duration, &status);
        bool passed = status == 1 && fabs(duration - expectedDuration) < 0.1 && frames > 0 && posterFrames > 0 && !badFrame && !lastPlaying && !lastSeeking &&
            running > 0.1 && fabs(still - paused) < 0.04 &&
            fabs(forward - 1.2) < 0.05 && fabs(backward - 0.2) < 0.05 &&
            forwardFrames > 0 && backwardFrames > 0 && rapidFrames > 0 && fabs(rapid - 1.0) < 0.05 && endFrames > 0 && fabs(end - expectedDuration) < 0.05 && replay > 0.1 && replay < 0.5 &&
            dragFrames >= 3 && settledFrames > 0 && !settledSeeking && fabs(settled - 1.4) < 0.05;
        printf("status=%d duration=%.3f size=%zux%zu frames=%d poster_frames=%d playing_time=%.3f paused_delta=%.6f forward=%.3f forward_frames=%d backward=%.3f backward_frames=%d rapid=%.3f rapid_frames=%d drag_frames=%d settled=%.3f settled_frames=%d end=%.3f end_frames=%d replay=%.3f result=%s\n",
            status, duration, width, height, frames, posterFrames, running, still-paused, forward, forwardFrames, backward, backwardFrames, rapid, rapidFrames, dragFrames, settled, settledFrames, end, endFrames, replay, passed ? "PASS" : "FAIL");
        ASPlaybackDestroy(player);
        return passed ? 0 : 1;
    }
}
