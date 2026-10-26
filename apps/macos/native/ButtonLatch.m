#import "AgentScreenPrivate.h"

#import <AppKit/AppKit.h>
#import <pthread.h>
#import <stdatomic.h>
#import <unistd.h>

// The left button as apps read it while an agent drags (#37).
//
// [NSEvent pressedMouseButtons] reads WindowServer's session button state,
// one for every process; Chromium and WebKit build a web pointer event's
// `buttons` from it. Events posted to one process don't change it, so a
// web page saw an agent's drag with no button held. A left press posted at
// the session tap sets it as WindowServer routes the event; dropped at the
// head of the annotated session tap, right after, it reaches no app. The
// press is at the cursor's own location, as an event posted there moves the
// cursor to its location.

static const int64_t SSLatchMarker = 0x4153424c; // "ASBL"

static CFMachPortRef SSLatchTap;
static pthread_mutex_t SSLatchLock = PTHREAD_MUTEX_INITIALIZER;
static int SSLatchDepth;
static bool SSLatchPosted;
// The user's own left button, from the hardware presses the tap sees.
static atomic_bool SSUserHolding;

static CGEventRef SSLatchCallback(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *refcon) {
    (void)proxy;
    (void)refcon;
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
        if (SSLatchTap) {
            CGEventTapEnable(SSLatchTap, true);
        }
        return event;
    }
    if (CGEventGetIntegerValueField(event, kCGEventSourceUserData) == SSLatchMarker) {
        return NULL;
    }
    // A hand's press has no posting process. Events any Agent Screen posts
    // to an agent's window pass here too, with its pid.
    if (CGEventGetIntegerValueField(event, kCGEventSourceUnixProcessID) == 0) {
        if (type == kCGEventLeftMouseDown) {
            atomic_store(&SSUserHolding, true);
        } else if (type == kCGEventLeftMouseUp) {
            atomic_store(&SSUserHolding, false);
        }
    }
    return event;
}

static void *SSLatchThread(void *ready) {
    CGEventMask mask = CGEventMaskBit(kCGEventLeftMouseDown) | CGEventMaskBit(kCGEventLeftMouseUp);
    CFMachPortRef tap = CGEventTapCreate(kCGAnnotatedSessionEventTap, kCGHeadInsertEventTap,
                                         kCGEventTapOptionDefault, mask, SSLatchCallback, NULL);
    if (tap) {
        CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(NULL, tap, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CFRelease(source);
        CGEventTapEnable(tap, true);
        SSLatchTap = tap;
    }
    dispatch_semaphore_signal((__bridge dispatch_semaphore_t)ready);
    if (tap) {
        CFRunLoopRun();
    }
    return NULL;
}

static bool SSLatchReady(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        pthread_t thread;
        if (pthread_create(&thread, NULL, SSLatchThread, (__bridge void *)ready) == 0) {
            pthread_detach(thread);
            dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
        }
    });
    return SSLatchTap && CGEventTapIsEnabled(SSLatchTap);
}

// Wait up to 250 ms for WindowServer to have the left button `held`.
static bool SSLatchSettle(bool held) {
    for (int i = 0; i < 125; i++) {
        if ((([NSEvent pressedMouseButtons] & 1) != 0) == held) {
            return true;
        }
        usleep(2000);
    }
    return false;
}

static void SSLatchPost(CGEventType type) {
    CGEventSourceRef source = CGEventSourceCreate(kCGEventSourceStatePrivate);
    CGEventRef here = CGEventCreate(NULL);
    CGPoint at = CGEventGetLocation(here);
    CFRelease(here);
    CGEventRef event = CGEventCreateMouseEvent(source, type, at, kCGMouseButtonLeft);
    if (source) {
        CFRelease(source);
    }
    if (!event) {
        return;
    }
    CGEventSetIntegerValueField(event, kCGEventSourceUserData, SSLatchMarker);
    CGEventPost(kCGSessionEventTap, event);
    CFRelease(event);
}

bool SSLatchHold(void) {
    pthread_mutex_lock(&SSLatchLock);
    bool held = true;
    if (SSLatchDepth++ == 0) {
        SSLatchPosted = false;
        // The user holds the button already: apps read it held.
        if (([NSEvent pressedMouseButtons] & 1) == 0) {
            // Without the tap the press would reach the app under the
            // user's cursor.
            if (SSLatchReady()) {
                SSLatchPost(kCGEventLeftMouseDown);
                SSLatchPosted = true;
                held = SSLatchSettle(true);
            } else {
                held = false;
            }
        }
    }
    pthread_mutex_unlock(&SSLatchLock);
    return held;
}

int SSLatchRelease(void) {
    int result = 0;
    pthread_mutex_lock(&SSLatchLock);
    if (SSLatchDepth > 0 && --SSLatchDepth == 0 && SSLatchPosted) {
        result = 3;
        SSLatchPosted = false;
        // The user pressed their own button meanwhile: their release lets
        // go, and a release now would end their drag. Posted with the tap
        // gone too: a release alone does less to the app under the cursor
        // than a button left held for everyone.
        if (!atomic_load(&SSUserHolding)) {
            SSLatchReady();
            SSLatchPost(kCGEventLeftMouseUp);
            result = SSLatchSettle(false) ? 1 : 2;
        }
    }
    pthread_mutex_unlock(&SSLatchLock);
    return result;
}
