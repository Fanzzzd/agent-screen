// Two owned native fixture apps on one Agent Display. Only explicitly
// tagged events are test data. Untagged input is escrowed, never printed.
#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <fcntl.h>
#import <sys/stat.h>
#include <unistd.h>
#include <time.h>
#include <signal.h>
#import "AgentScreenPrivate.h"

extern int SSMenuKeyPid(void);
extern int32_t SSMenuStealFocus(int32_t *);
extern int32_t SSMenuReleaseFocusID(int32_t);
static const uint32_t AckMagic = 0x47524451;
static NSDictionary *Env;
static NSString *LogPath, *CommandPath;
static uint32_t Tag;
static pid_t MenuPid, HelperPid;
static BOOL UserRole, Secure;
static int32_t Lease;
static pid_t OriginalKeyPid;
static NSUInteger Packet, Received;
static NSString *Case;
static CGRect Area;
static CGWindowID WindowID, MenuWindow;
static NSWindow *Window;
static dispatch_source_t SignalTwo;
static volatile sig_atomic_t StopEmission;
static void stopEmission(int s) { (void)s; StopEmission = 1; }

static NSString *T(NSString *en, NSString *zh) { return [Env[@"AGENTSCREEN_LANG"] isEqual:@"zh"] ? zh : en; }
static void append(NSDictionary *record) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:record options:0 error:NULL];
    int fd = open(LogPath.fileSystemRepresentation, O_WRONLY|O_APPEND|O_CREAT|O_NOFOLLOW, 0600);
    if (fd >= 0) { write(fd, data.bytes, data.length); write(fd, "\n", 1); close(fd); }
}
static void say(NSString *kind, NSDictionary *extra) {
    NSMutableDictionary *r = [@{@"kind":kind,@"case":Case ?: @"",@"t":@(clock_gettime_nsec_np(CLOCK_UPTIME_RAW)),@"pid":@(getpid())} mutableCopy];
    [r addEntriesFromDictionary:extra]; append(r);
}
static NSDictionary *command(void) {
    NSData *data = [NSData dataWithContentsOfFile:CommandPath];
    return data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : @{};
}
static CGRect cgFrame(CGWindowID number) {
    NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, number));
    for (NSDictionary *w in windows) {
        if ([w[(__bridge NSString *)kCGWindowNumber] unsignedIntValue] == number && [w[(__bridge NSString *)kCGWindowOwnerPID] intValue] == getpid()) {
            CGRect rect; if (CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)w[(__bridge NSString *)kCGWindowBounds], &rect)) return rect;
        }
    }
    return CGRectZero;
}
static BOOL marked(CGEventRef e) {
    uint64_t tag = CGEventGetIntegerValueField(e, kCGEventSourceUserData);
    pid_t pid = (pid_t)CGEventGetIntegerValueField(e, kCGEventSourceUnixProcessID);
    return (pid == getpid() && (uint32_t)(tag >> 32) == Tag) ||
        (pid == HelperPid && (uint32_t)(tag >> 32) == (Tag ^ AckMagic) && (tag & 1));
}
static void releaseLease(void) {
    if (Secure) { DisableSecureEventInput(); Secure = NO; }
    if (Lease) { SSMenuReleaseFocusID(Lease); Lease = 0; }
}
static void unexpected(CGEventRef e) {
    // Preserve, rather than replay, ambiguous untagged input. The live
    // driver stops the session and does not discard this private custody.
    NSData *data = CFBridgingRelease(CGEventCreateData(NULL, e));
    NSString *path = [LogPath stringByAppendingString:@".unexpected"];
    int fd = open(path.fileSystemRepresentation, O_WRONLY|O_APPEND|O_CREAT|O_NOFOLLOW, 0600);
    if (fd >= 0) {
        NSData *record = [NSJSONSerialization dataWithJSONObject:@{@"event":[data base64EncodedStringWithOptions:0]} options:0 error:NULL];
        write(fd, record.bytes, record.length); write(fd, "\n", 1); fsync(fd); close(fd);
    }
    say(@"unexpected-input", @{});
}
static BOOL ownFocus(void) {
    pid_t p = SSMenuKeyPid();
    return p == getpid() || (MenuPid > 0 && p == MenuPid) || (HelperPid > 0 && p == HelperPid);
}
static BOOL sourceWindowValid(void) { return !CGRectIsEmpty(cgFrame(WindowID)) && CGRectContainsRect(Area, cgFrame(WindowID)); }
static BOOL post(CGEventRef e) {
    if (StopEmission || !ownFocus() || !sourceWindowValid() || IsSecureEventInputEnabled()) { say(@"emit-refused", @{}); return NO; }
    CGEventSetIntegerValueField(e, kCGEventSourceUserData, ((uint64_t)Tag << 32) | Packet);
    CGEventSetTimestamp(e, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
    if (StopEmission) return NO;
    CGEventPost(kCGHIDEventTap, e); Packet++;
    return YES;
}

@interface GuardView : NSView
@end
@implementation GuardView
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent *)event { (void)event; return YES; }
- (void)keyDown:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event.CGEvent); return; }
    Received++;
    uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW), then = CGEventGetTimestamp(event.CGEvent);
    say(@"key", @{@"text":event.characters ?: @"",@"key":@(Window.isKeyWindow),@"latencyNs":@(now >= then ? now-then : 0),@"received":@(Received)});
}
- (void)mouseDown:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event.CGEvent); return; }
    say(@"down", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
- (void)mouseUp:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event.CGEvent); return; }
    say(@"up", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
- (void)scrollWheel:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event.CGEvent); return; }
    say(@"scroll", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
@end

@interface GuardApplication : NSApplication
@end
@implementation GuardApplication
- (void)sendEvent:(NSEvent *)event {
    if (UserRole && (event.type == NSEventTypeKeyDown || event.type == NSEventTypeKeyUp || event.type == NSEventTypeFlagsChanged || event.type == NSEventTypeLeftMouseDown || event.type == NSEventTypeLeftMouseUp || event.type == NSEventTypeScrollWheel)) {
        if (!marked(event.CGEvent)) { unexpected(event.CGEvent); return; }
        if (event.type == NSEventTypeKeyUp) say(@"key-up", @{});
        if ((event.type == NSEventTypeLeftMouseDown || event.type == NSEventTypeLeftMouseUp || event.type == NSEventTypeScrollWheel) && event.window != Window) {
            say(@"wrong-window", @{}); return;
        }
    }
    [super sendEvent:event];
}
@end

@interface GuardDelegate : NSObject <NSApplicationDelegate, NSWindowDelegate, NSMenuDelegate>
@property NSPopUpButton *popup;
@property NSUInteger generation;
@end
@implementation GuardDelegate
- (void)windowDidResignKey:(NSNotification *)note { (void)note; if (UserRole) say(@"resign-key", @{}); }
- (void)menuWillOpen:(NSMenu *)menu {
    self.generation++; NSUInteger generation = self.generation;
    say(@"menu-open", @{});
    [NSThread sleepForTimeInterval:0.08];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 600*NSEC_PER_MSEC), dispatch_get_main_queue(), ^{ if (self.generation == generation) [menu cancelTrackingWithoutAnimation]; });
}
- (void)menuDidClose:(NSMenu *)menu { (void)menu; self.generation++; say(@"menu-close", @{}); }
- (void)clickedA:(id)sender { (void)sender; say(@"button", @{@"side":@"A"}); }
- (void)clickedB:(id)sender { (void)sender; say(@"button", @{@"side":@"B"}); }
- (void)picked:(NSPopUpButton *)popup { say(@"picked", @{@"title":popup.titleOfSelectedItem ?: @""}); }
- (void)takeLease {
    [Window makeKeyWindow]; SSFocusWithoutRaise(getpid(), WindowID);
    if (!Lease) {
        OriginalKeyPid = SSMenuKeyPid();
        if (SSMenuStealFocus(&Lease) != 0) { say(@"lease-failed", @{}); return; }
    }
    say(@"leased", @{@"key":@(Window.isKeyWindow),@"focus":@(SSMenuKeyPid()),@"original":@(OriginalKeyPid)});
}
- (void)runCommand {
    NSDictionary *r = command();
    NSString *cmd = r[@"command"];
    if ([cmd isEqual:@"release"]) { releaseLease(); say(@"released", @{@"focus":@(SSMenuKeyPid())}); return; }
    if ([cmd isEqual:@"secure-on"]) { if (!Secure) { OSStatus s = EnableSecureEventInput(); Secure = s == noErr; say(@"secure-on", @{@"status":@(s)}); } return; }
    if ([cmd isEqual:@"secure-off"]) { if (Secure) { DisableSecureEventInput(); Secure = NO; } say(@"secure-off", @{}); return; }
    if ([cmd isEqual:@"where"]) { CGEventRef e = CGEventCreate(NULL); CGPoint p = CGEventGetLocation(e); CFRelease(e); say(@"where", @{@"x":@(p.x),@"y":@(p.y)}); return; }
    if ([cmd isEqual:@"restore-pointer"]) { CGPoint p = CGPointMake([r[@"x"] doubleValue],[r[@"y"] doubleValue]); CGError e = CGWarpMouseCursorPosition(p); say(@"restored-pointer", @{@"status":@(e)}); return; }
    Case = r[@"case"] ?: @"";
    Tag = [r[@"nonce"] unsignedIntValue]; HelperPid = [r[@"helper"] intValue]; MenuPid = [r[@"menu"] intValue]; MenuWindow = [r[@"menuWindow"] unsignedIntValue];
    Packet = 0; Received = 0; StopEmission = 0;
    if ([cmd isEqual:@"lease"]) { [self takeLease]; return; }
    if ([cmd isEqual:@"keys"]) {
        NSString *text = r[@"text"] ?: @"abcdefghijklmnopqrst";
        __block NSUInteger index = 0;
        __block NSTimer *timer;
        timer = [NSTimer scheduledTimerWithTimeInterval:0.005 repeats:YES block:^(NSTimer *t) {
            if (StopEmission) { [t invalidate]; timer = nil; say(@"stopped", @{@"packets":@(Packet)}); return; }
            if (IsSecureEventInputEnabled()) return;
            if (index >= text.length) { [t invalidate]; timer = nil; say(@"sent", @{@"packets":@(Packet)}); return; }
            if (!ownFocus() || !sourceWindowValid()) { [t invalidate]; timer = nil; say(@"emit-refused", @{}); return; }
            UniChar c = [text characterAtIndex:index++];
            CGEventSourceRef source = CGEventSourceCreate(kCGEventSourceStatePrivate);
            for (int down=1;down>=0;down--) {
                CGEventRef e = CGEventCreateKeyboardEvent(source, 0, down);
                CGEventKeyboardSetUnicodeString(e, 1, &c); CGEventSetFlags(e, 0); post(e); CFRelease(e);
            }
            CFRelease(source);
        }];
        return;
    }
    if ([cmd isEqual:@"mouse"]) {
        NSPoint cocoaA = [Window convertPointToScreen:NSMakePoint(100, 200)], cocoaB = [Window convertPointToScreen:NSMakePoint(360, 200)];
        NSScreen *screen = Window.screen;
        CGPoint a = CGPointMake(cocoaA.x, Area.origin.y + NSMaxY(screen.frame) - cocoaA.y);
        CGPoint b = CGPointMake(cocoaB.x, Area.origin.y + NSMaxY(screen.frame) - cocoaB.y);
        CGEventSourceRef source = CGEventSourceCreate(kCGEventSourceStatePrivate);
        CGEventRef move = CGEventCreateMouseEvent(source, kCGEventMouseMoved, a, kCGMouseButtonLeft); post(move); CFRelease(move);
        for (int down=1;down>=0;down--) {
            CGEventRef e = CGEventCreateMouseEvent(source, down ? kCGEventLeftMouseDown : kCGEventLeftMouseUp, a, kCGMouseButtonLeft);
            CGEventSetIntegerValueField(e, kCGMouseEventClickState, 1); post(e); CFRelease(e);
        }
        CGEventRef wheel = CGEventCreateScrollWheelEvent(source, kCGScrollEventUnitLine, 1, 1); CGEventSetLocation(wheel, a); post(wheel); CFRelease(wheel);
        move = CGEventCreateMouseEvent(source, kCGEventMouseMoved, b, kCGMouseButtonLeft); post(move); CFRelease(move);
        CFRelease(source);
        say(@"sent", @{@"packets":@(Packet),@"pointerX":@(b.x),@"pointerY":@(b.y)});
    }
}
- (void)applicationDidFinishLaunching:(NSNotification *)note {
    (void)note;
    CGDirectDisplayID display = [Env[@"GUARD_DISPLAY"] unsignedIntValue];
    NSScreen *screen = nil;
    for (NSScreen *s in NSScreen.screens) if ([s.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == display) screen = s;
    if (!screen || CGDisplayVendorNumber(display) != 0x5353) exit(3);
    Area = CGDisplayBounds(display);
    NSSize size = UserRole ? NSMakeSize(480,330) : NSMakeSize(300,220);
    Window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,size.width,size.height) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO screen:screen];
    [Window setFrameOrigin:NSMakePoint(screen.frame.origin.x + (UserRole ? 50 : 650), screen.frame.origin.y + (UserRole ? 60 : 350))];
    if (!NSContainsRect(screen.frame, Window.frame)) exit(4);
    Window.title = UserRole ? T(@"Guard user", @"测试输入") : T(@"Guard menu", @"测试菜单");
    Window.delegate = self; Window.acceptsMouseMovedEvents = YES;
    if (UserRole) {
        GuardView *view = [[GuardView alloc] initWithFrame:NSMakeRect(0,0,size.width,size.height)]; Window.contentView = view;
        NSButton *a = [[NSButton alloc] initWithFrame:NSMakeRect(20,150,200,100)]; a.title = T(@"Button A",@"按钮 A"); a.bezelStyle = NSBezelStyleRegularSquare; a.target = self; a.action = @selector(clickedA:); [view addSubview:a];
        NSButton *b = [[NSButton alloc] initWithFrame:NSMakeRect(260,150,200,100)]; b.title = T(@"Button B",@"按钮 B"); b.bezelStyle = NSBezelStyleRegularSquare; b.target = self; b.action = @selector(clickedB:); [view addSubview:b];
        [Window makeFirstResponder:view];
    }
    else {
        self.popup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(30,140,200,28) pullsDown:NO];
        [self.popup addItemsWithTitles:@[@"Apple",@"Banana",@"Cherry"]]; self.popup.menu.delegate = self;
        self.popup.target = self; self.popup.action = @selector(picked:); [Window.contentView addSubview:self.popup];
    }
    [NSApp unhideWithoutActivation];
    [Window orderFrontRegardless]; [Window displayIfNeeded];
    signal(SIGUSR1,stopEmission); signal(SIGUSR2,SIG_IGN);
    SignalTwo = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL,SIGUSR2,0,dispatch_get_main_queue());
    dispatch_source_set_event_handler(SignalTwo, ^{ [self runCommand]; }); dispatch_resume(SignalTwo);
    // Complete launch/unhide before publishing a window identity. A deferred
    // windowNumber of -1 is never a ready event or a containment sample.
    dispatch_async(dispatch_get_main_queue(), ^{
        [NSApp unhideWithoutActivation]; [Window orderFrontRegardless]; [Window displayIfNeeded];
        NSInteger number = Window.windowNumber;
        if (number <= 0 || number == UINT32_MAX || !Window.isVisible) { say(@"startup-failed", @{@"window":@(number),@"visible":@(Window.isVisible)}); exit(6); }
        WindowID = (CGWindowID)number;
        CGRect frame = cgFrame(WindowID);
        if (CGRectIsEmpty(frame) || !CGRectContainsRect(Area,frame)) { say(@"startup-failed", @{@"window":@(number)}); exit(7); }
        NSPoint cocoa = [Window convertPointToScreen:NSMakePoint(130,154)];
        say(@"ready", @{@"window":@(WindowID),@"areaX":@(Area.origin.x),@"areaY":@(Area.origin.y),@"width":@(Area.size.width),@"height":@(Area.size.height),@"x":@(frame.origin.x),@"y":@(frame.origin.y),@"controlX":@(cocoa.x),@"controlY":@(Area.origin.y+NSMaxY(screen.frame)-cocoa.y)});
        [NSTimer scheduledTimerWithTimeInterval:0.02 repeats:YES block:^(NSTimer *t) { (void)t; if (!CGDisplayIsOnline(display) || !CGRectContainsRect(Area,cgFrame(WindowID))) { releaseLease(); exit(5); } }];
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,55*NSEC_PER_SEC),dispatch_get_main_queue(), ^{ releaseLease(); exit(0); });
}
@end
static GuardDelegate *Delegate;
int main(void) { @autoreleasepool {
    Env = NSProcessInfo.processInfo.environment; UserRole = [Env[@"GUARD_ROLE"] isEqual:@"user"];
    LogPath = Env[@"GUARD_LOG"]; CommandPath = Env[@"GUARD_COMMAND"];
    if (!LogPath || !CommandPath) return 2;
    [[NSUserDefaults standardUserDefaults] registerDefaults:@{@"NSMenuDisableOutOfProcessMenusDueToIncompatibility":@YES}];
    GuardApplication *app = [GuardApplication sharedApplication]; [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
    Delegate = [GuardDelegate new]; app.delegate = Delegate; [app run]; releaseLease(); return 0;
} }
