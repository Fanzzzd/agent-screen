// Local AVPlayer output for GPUI. No windows or input APIs.
#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#import <math.h>
#import <dispatch/dispatch.h>

@interface ASPlayback : NSObject
@property(nonatomic, strong) AVPlayer *player;
@property(nonatomic, strong) AVPlayerItemVideoOutput *output;
@property(nonatomic) uint64_t seekGeneration;
@property(nonatomic) bool seeking;
@property(nonatomic) bool forceFrame;
@end
@implementation ASPlayback
@end

void *ASPlaybackCreate(const char *path) {
    @autoreleasepool {
        NSString *file = [NSString stringWithUTF8String:path];
        if (!file) return NULL;
        // Always a local file URL. Sidecars never supply media URLs.
        AVPlayerItem *item = [AVPlayerItem playerItemWithURL:[NSURL fileURLWithPath:file]];
        NSDictionary *attributes = @{
            (id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
            (id)kCVPixelBufferMetalCompatibilityKey: @YES,
            (id)kCVPixelBufferIOSurfacePropertiesKey: @{}
        };
        ASPlayback *state = [ASPlayback new];
        state.output = [[AVPlayerItemVideoOutput alloc] initWithPixelBufferAttributes:attributes];
        [item addOutput:state.output];
        state.player = [AVPlayer playerWithPlayerItem:item];
        state.forceFrame = true;
        return (__bridge_retained void *)state;
    }
}

void ASPlaybackDestroy(void *raw) {
    @autoreleasepool {
        ASPlayback *state = (__bridge_transfer ASPlayback *)raw;
        [state.player pause];
        [state.player.currentItem removeOutput:state.output];
        [state.player.currentItem cancelPendingSeeks];
        [state.player replaceCurrentItemWithPlayerItem:nil];
    }
}

void ASPlaybackSetPlaying(void *raw, bool playing) {
    ASPlayback *state = (__bridge ASPlayback *)raw;
    if (playing) [state.player play]; else [state.player pause];
}

void ASPlaybackSeek(void *raw, double seconds) {
    ASPlayback *state = (__bridge ASPlayback *)raw;
    if (!isfinite(seconds) || seconds < 0) return;
    uint64_t generation = ++state.seekGeneration;
    state.seeking = true;
    [state.player seekToTime:CMTimeMakeWithSeconds(seconds, 600)
            toleranceBefore:kCMTimeZero toleranceAfter:kCMTimeZero
          completionHandler:^(BOOL finished) {
        (void)finished;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (state.seekGeneration == generation) {
                state.seeking = false;
                state.forceFrame = true;
            }
        });
    }];
}

// Returned pixel buffer follows the create rule. One captured AVPlayer
// media time drives both the requested video frame and the pointer sample.
CVPixelBufferRef ASPlaybackSnapshot(void *raw, double *seconds, double *duration,
                                    int *status, bool *playing, bool *seeking) {
    @autoreleasepool {
        ASPlayback *state = (__bridge ASPlayback *)raw;
        CMTime time = state.player.currentTime;
        *seconds = CMTIME_IS_NUMERIC(time) ? CMTimeGetSeconds(time) : 0;
        CMTime length = state.player.currentItem.duration;
        *duration = CMTIME_IS_NUMERIC(length) ? CMTimeGetSeconds(length) : 0;
        *status = state.player.currentItem.status == AVPlayerItemStatusFailed ? 2 :
                  state.player.currentItem.status == AVPlayerItemStatusReadyToPlay ? 1 : 0;
        *playing = state.player.rate != 0;
        *seeking = state.seeking;
        if (state.seeking) return NULL;
        if (!state.forceFrame && ![state.output hasNewPixelBufferForItemTime:time]) return NULL;
        CVPixelBufferRef frame = [state.output copyPixelBufferForItemTime:time itemTimeForDisplay:NULL];
        if (frame) state.forceFrame = false;
        return frame;
    }
}
