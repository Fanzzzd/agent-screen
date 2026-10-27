// Two owned native fixture apps on one Agent Display. Only explicitly
// tagged events are test data. Untagged input is escrowed, never printed.
#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <fcntl.h>
#import <sys/stat.h>
#include <unistd.h>
#include <time.h>
#include <signal.h>
#include <errno.h>
#include <assert.h>
#include <poll.h>
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
static dispatch_source_t SignalTwo, SignalTerm, SignalInterrupt;
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
// An unexpected NSEvent/CGEvent stays strongly owned until an atomic,
// synced per-event record is durable. No failure path replays it elsewhere.
typedef struct {
    NSData *(*encode)(CGEventRef);
    int (*openFile)(const char *, int, mode_t);
    ssize_t (*writeBytes)(int, const void *, size_t);
    int (*syncFile)(int);
    int (*closeFile)(int);
    int (*renameFile)(const char *, const char *);
    int (*unlinkFile)(const char *);
} CustodyIO;
static NSData *encodeUnexpected(CGEventRef e) { return e ? CFBridgingRelease(CGEventCreateData(NULL,e)) : nil; }
static int openUnexpected(const char *p,int flags,mode_t mode) { return open(p,flags,mode); }
static CustodyIO RealCustodyIO(void) { return (CustodyIO){encodeUnexpected,openUnexpected,write,fsync,close,rename,unlink}; }
static BOOL writeFully(CustodyIO io,int fd,const void *bytes,size_t length) {
    const uint8_t *p=bytes;
    while (length) {
        ssize_t n=io.writeBytes(fd,p,length);
        if (n<0 && errno==EINTR) continue;
        if (n<=0 || (size_t)n>length) { if (n==0) errno=EIO; return NO; }
        p+=n; length-=(size_t)n;
    }
    return YES;
}
@interface UnexpectedEntry : NSObject {
    CGEventRef _original;
}
@property(strong) NSEvent *nativeEvent;
@property NSUInteger number;
- (instancetype)initNative:(NSEvent *)event;
- (instancetype)initCG:(CGEventRef)event;
- (CGEventRef)original;
@end
@implementation UnexpectedEntry
- (instancetype)initNative:(NSEvent *)event { if ((self=[super init])) { self.nativeEvent=event; CGEventRef original=event.CGEvent; if (original) _original=(CGEventRef)CFRetain(original); } return self; }
- (instancetype)initCG:(CGEventRef)event { if ((self=[super init]) && event) _original=(CGEventRef)CFRetain(event); return self; }
- (CGEventRef)original { if (!_original) { CGEventRef original=self.nativeEvent.CGEvent; if (original) _original=(CGEventRef)CFRetain(original); } return _original; }
- (void)dealloc { if (_original) CFRelease(_original); }
@end
@interface UnexpectedCustody : NSObject
@property(strong) NSMutableArray<UnexpectedEntry *> *pending;
@property(copy) NSString *prefix;
@property CustodyIO io;
@property NSUInteger next;
@property NSUInteger attempt;
@property int lastError;
- (instancetype)initPrefix:(NSString *)prefix io:(CustodyIO)io;
- (void)captureNative:(NSEvent *)event;
- (void)captureCG:(CGEventRef)event;
- (BOOL)flush;
@end
@implementation UnexpectedCustody
- (instancetype)initPrefix:(NSString *)prefix io:(CustodyIO)io { if ((self=[super init])) { self.pending=[NSMutableArray new]; self.prefix=prefix; self.io=io; } return self; }
- (void)captureNative:(NSEvent *)event { UnexpectedEntry *entry=[[UnexpectedEntry alloc] initNative:event]; entry.number=self.next++; [self.pending addObject:entry]; }
- (void)captureCG:(CGEventRef)event { UnexpectedEntry *entry=[[UnexpectedEntry alloc] initCG:event]; entry.number=self.next++; [self.pending addObject:entry]; }
- (BOOL)persist:(UnexpectedEntry *)entry {
    NSData *encoded=nil, *record=nil;
    @try {
        CGEventRef original=entry.original;
        if (!original || !(encoded=self.io.encode(original)) || !encoded.length) { self.lastError=EINVAL; return NO; }
        NSString *payload=[encoded base64EncodedStringWithOptions:0];
        NSError *error=nil;
        if (payload) record=[NSJSONSerialization dataWithJSONObject:@{@"version":@1,@"event":payload} options:0 error:&error];
        if (!record || error) { self.lastError=EINVAL; return NO; }
    } @catch (NSException *exception) { (void)exception; self.lastError=EINVAL; return NO; }
    NSString *final=[self.prefix stringByAppendingFormat:@".unexpected-%d-%lu.json",getpid(),(unsigned long)entry.number];
    NSString *temporary=[final stringByAppendingFormat:@".writing-%lu",(unsigned long)self.attempt++];
    CustodyIO io=self.io;
    int fd=io.openFile(temporary.fileSystemRepresentation,O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW,0600);
    if (fd<0) { self.lastError=errno; return NO; }
    BOOL written=writeFully(io,fd,record.bytes,record.length) && writeFully(io,fd,"\n",1);
    int failure=written ? 0 : (errno ?: EIO);
    if (!failure && io.syncFile(fd)<0) failure=errno ?: EIO;
    if (io.closeFile(fd)<0 && !failure) failure=errno ?: EIO;
    if (!failure && io.renameFile(temporary.fileSystemRepresentation,final.fileSystemRepresentation)<0) failure=errno ?: EIO;
    if (!failure) {
        int directory=io.openFile(final.stringByDeletingLastPathComponent.fileSystemRepresentation,O_RDONLY|O_DIRECTORY|O_NOFOLLOW,0);
        if (directory<0) failure=errno ?: EIO;
        else { if (io.syncFile(directory)<0) failure=errno ?: EIO; if (io.closeFile(directory)<0 && !failure) failure=errno ?: EIO; }
    }
    if (failure) { io.unlinkFile(temporary.fileSystemRepresentation); self.lastError=failure; return NO; }
    self.lastError=0; return YES;
}
- (BOOL)flush {
    while (self.pending.count) {
        UnexpectedEntry *entry=self.pending.firstObject;
        if (![self persist:entry]) return NO;
        [self.pending removeObjectAtIndex:0];
    }
    return YES;
}
@end
static BOOL custodyAllowsExit(UnexpectedCustody *custody) { return !custody || [custody flush]; }
static UnexpectedCustody *Unexpected;
static BOOL CustodyRetry;
static int RequestedExit=-1;
static void requestExit(int code) {
    StopEmission=1; releaseLease();
    if (custodyAllowsExit(Unexpected)) exit(code);
    // A storage failure is an explicit exception to the 55-second fuse.
    // Keep the event owner alive, stop input emission and refuse termination.
    RequestedExit=code;
    Window.title=T(@"Input recovery pending",@"输入恢复待处理");
    say(@"custody-pending",@{@"retained":@(Unexpected.pending.count),@"errno":@(Unexpected.lastError)});
    if (!CustodyRetry) {
        CustodyRetry=YES;
        [NSTimer scheduledTimerWithTimeInterval:1 repeats:YES block:^(NSTimer *timer) {
            if ([Unexpected flush]) { [timer invalidate]; CustodyRetry=NO; if (RequestedExit>=0) exit(RequestedExit); }
        }];
    }
}
static void unexpected(NSEvent *event) {
    if (!Unexpected) Unexpected=[[UnexpectedCustody alloc] initPrefix:LogPath io:RealCustodyIO()];
    [Unexpected captureNative:event]; // owns the original before fallible IO
    StopEmission=1; releaseLease();
    BOOL durable=[Unexpected flush];
    say(@"unexpected-input",@{@"durable":@(durable),@"retained":@(Unexpected.pending.count),@"errno":@(Unexpected.lastError)});
    if (!durable) requestExit(8);
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
    if (!marked(event.CGEvent)) { unexpected(event); return; }
    Received++;
    uint64_t now = clock_gettime_nsec_np(CLOCK_UPTIME_RAW), then = CGEventGetTimestamp(event.CGEvent);
    say(@"key", @{@"text":event.characters ?: @"",@"key":@(Window.isKeyWindow),@"latencyNs":@(now >= then ? now-then : 0),@"received":@(Received)});
}
- (void)mouseDown:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event); return; }
    say(@"down", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
- (void)mouseUp:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event); return; }
    say(@"up", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
- (void)scrollWheel:(NSEvent *)event {
    if (!marked(event.CGEvent)) { unexpected(event); return; }
    say(@"scroll", @{@"side":event.locationInWindow.x < 240 ? @"A" : @"B",@"window":@(event.window.windowNumber)});
}
@end

@interface GuardApplication : NSApplication
@end
@implementation GuardApplication
- (void)sendEvent:(NSEvent *)event {
    if (UserRole && (event.type == NSEventTypeKeyDown || event.type == NSEventTypeKeyUp || event.type == NSEventTypeFlagsChanged || event.type == NSEventTypeLeftMouseDown || event.type == NSEventTypeLeftMouseUp || event.type == NSEventTypeScrollWheel)) {
        if (!marked(event.CGEvent)) { unexpected(event); return; }
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
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)app { (void)app; StopEmission=1; releaseLease(); return custodyAllowsExit(Unexpected) ? NSTerminateNow : NSTerminateCancel; }
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
    signal(SIGTERM,SIG_IGN); signal(SIGINT,SIG_IGN);
    SignalTerm=dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL,SIGTERM,0,dispatch_get_main_queue()); dispatch_source_set_event_handler(SignalTerm, ^{ requestExit(0); }); dispatch_resume(SignalTerm);
    SignalInterrupt=dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL,SIGINT,0,dispatch_get_main_queue()); dispatch_source_set_event_handler(SignalInterrupt, ^{ requestExit(0); }); dispatch_resume(SignalInterrupt);
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
        [NSTimer scheduledTimerWithTimeInterval:0.02 repeats:YES block:^(NSTimer *t) { (void)t; if (!CGDisplayIsOnline(display) || !CGRectContainsRect(Area,cgFrame(WindowID))) { [Window orderOut:nil]; requestExit(5); } }];
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,55*NSEC_PER_SEC),dispatch_get_main_queue(), ^{ requestExit(0); });
}
@end
// Fault tests construct CGEvent values only. They never initialize NSApp,
// install a tap, create a window, post input, or touch a display.
typedef NS_ENUM(int, CustodyFault) { CFNone, CFEncodeNil, CFEncodeThrow, CFOpenDir, CFNoSpace, CFShortFail, CFSyncFail, CFRenameFail, CFZeroWrite, CFEintr, CFCloseFail, CFDirSyncFail };
static CustodyFault TestFault;
static NSUInteger TestWrites, TestOpens, TestBytes, TestSyncs, TestCloses;
static NSData *testEncode(CGEventRef e) { if (TestFault==CFEncodeNil) return nil; if (TestFault==CFEncodeThrow) @throw [NSException exceptionWithName:@"encode fault" reason:@"offline injection" userInfo:nil]; return encodeUnexpected(e); }
static int testOpen(const char *p,int f,mode_t m) { (void)p;(void)f;(void)m; TestOpens++; if (TestFault==CFOpenDir) { errno=EISDIR; return -1; } return 300; }
static ssize_t testWrite(int fd,const void *p,size_t n) {
    (void)fd;(void)p; TestWrites++;
    if (TestFault==CFNoSpace || (TestFault==CFShortFail && TestWrites>1)) { errno=ENOSPC; return -1; }
    if (TestFault==CFZeroWrite) return 0;
    if (TestFault==CFEintr && TestWrites==1) { errno=EINTR; return -1; }
    size_t count=MIN(n,3); TestBytes+=count; return (ssize_t)count;
}
static int testSync(int fd) { (void)fd; TestSyncs++; if (TestFault==CFSyncFail || (TestFault==CFDirSyncFail && TestSyncs==2)) { errno=EIO; return -1; } return 0; }
static int testClose(int fd) { (void)fd; TestCloses++; if (TestFault==CFCloseFail && TestCloses==1) { errno=EIO; return -1; } return 0; }
static int testRename(const char *a,const char *b) { (void)a;(void)b; if (TestFault==CFRenameFail) { errno=EACCES; return -1; } return 0; }
static int testUnlink(const char *p) { (void)p; return 0; }
static CustodyIO TestIO(void) { return (CustodyIO){testEncode,testOpen,testWrite,testSync,testClose,testRename,testUnlink}; }
@interface NilCGNativeEvent : NSObject
- (CGEventRef)CGEvent;
@end
@implementation NilCGNativeEvent
- (CGEventRef)CGEvent { return NULL; }
@end
static int custodySelfTest(void) {
    CustodyFault failures[]={CFEncodeNil,CFEncodeThrow,CFOpenDir,CFNoSpace,CFShortFail,CFSyncFail,CFRenameFail,CFZeroWrite,CFCloseFail,CFDirSyncFail};
    for (NSUInteger i=0;i<sizeof(failures)/sizeof(failures[0]);i++) {
        TestFault=failures[i]; TestWrites=TestOpens=TestBytes=TestSyncs=TestCloses=0;
        CGEventSourceRef source=CGEventSourceCreate(kCGEventSourceStatePrivate);
        CGEventRef original=CGEventCreateKeyboardEvent(source,0,true); CGEventSetTimestamp(original,777);
        UnexpectedCustody *custody=[[UnexpectedCustody alloc] initPrefix:@"/not-written/offline" io:TestIO()];
        [custody captureCG:original]; CFRelease(original); CFRelease(source);
        assert(!custodyAllowsExit(custody));
        assert(custody.pending.count==1);
        assert(CGEventGetTimestamp(custody.pending.firstObject.original)==777);
        if (TestFault==CFEncodeNil || TestFault==CFEncodeThrow) assert(TestOpens==0);
        if (TestFault==CFShortFail) assert(TestWrites==2 && TestBytes==3);
        TestFault=CFNone; TestWrites=TestSyncs=TestCloses=0;
        assert(custodyAllowsExit(custody)); assert(custody.pending.count==0);
    }
    for (int mode=0;mode<2;mode++) {
        TestFault=mode ? CFEintr : CFNone; TestWrites=TestOpens=TestBytes=TestSyncs=TestCloses=0;
        CGEventRef event=CGEventCreateKeyboardEvent(NULL,0,true);
        UnexpectedCustody *custody=[[UnexpectedCustody alloc] initPrefix:@"/not-written/offline" io:TestIO()]; [custody captureCG:event]; CFRelease(event);
        assert(custodyAllowsExit(custody)); assert(custody.pending.count==0 && TestWrites>2 && TestBytes>3);
    }
    // A native event whose CGEvent is unavailable is still strongly owned.
    @autoreleasepool {
        UnexpectedCustody *custody=[[UnexpectedCustody alloc] initPrefix:@"/not-written/offline" io:TestIO()];
        NilCGNativeEvent *native=[NilCGNativeEvent new]; __weak id weak=native;
        [custody captureNative:(NSEvent *)native]; native=nil;
        assert(weak!=nil && !custodyAllowsExit(custody) && custody.pending.count==1);
        [custody.pending removeAllObjects]; // discard only this artificial object
    }
    // Real filesystem happy path, still without NSApp or any input posting.
    char directory[]="/tmp/agent-screen-unexpected-XXXXXX"; assert(mkdtemp(directory));
    NSString *root=[NSString stringWithUTF8String:directory];
    UnexpectedCustody *custody=[[UnexpectedCustody alloc] initPrefix:[root stringByAppendingPathComponent:@"input"] io:RealCustodyIO()];
    CGEventRef event=CGEventCreateKeyboardEvent(NULL,0,true); CGEventSetTimestamp(event,888);
    [custody captureCG:event]; CFRelease(event);
    assert(custodyAllowsExit(custody) && custody.pending.count==0);
    NSArray *files=[[NSFileManager defaultManager] contentsOfDirectoryAtPath:root error:NULL]; assert(files.count==1);
    NSString *path=[root stringByAppendingPathComponent:files.firstObject];
    struct stat mode; assert(lstat(path.fileSystemRepresentation,&mode)==0 && (mode.st_mode & 0777)==0600);
    NSDictionary *record=[NSJSONSerialization JSONObjectWithData:[NSData dataWithContentsOfFile:path] options:0 error:NULL];
    NSData *bytes=[[NSData alloc] initWithBase64EncodedString:record[@"event"] options:0];
    CGEventRef restored=CGEventCreateFromData(NULL,(__bridge CFDataRef)bytes); assert(restored && CGEventGetTimestamp(restored)==888); CFRelease(restored);
    assert([[NSFileManager defaultManager] removeItemAtPath:root error:NULL]);
    puts("14 offline unexpected-input custody cases passed"); return 0;
}
// This subprocess owns one artificial event created here, never live
// input. TERM cannot end it while the injected storage failure persists.
// A private controller repair command (or controller EOF during test
// teardown) removes only the artificial fault, then drains before exit.
static volatile sig_atomic_t TestTerm;
static void testTerm(int s) { (void)s; TestTerm=1; }
static int custodyTermChild(void) {
    TestFault=CFNoSpace;
    UnexpectedCustody *custody=[[UnexpectedCustody alloc] initPrefix:@"/not-written/process-test" io:TestIO()];
    CGEventRef event=CGEventCreateKeyboardEvent(NULL,0,true); [custody captureCG:event]; CFRelease(event);
    signal(SIGTERM,testTerm);
    puts("ready retained=1"); fflush(stdout);
    for (;;) {
        if (TestTerm) {
            TestTerm=0;
            if (custodyAllowsExit(custody)) return 0;
            printf("term-refused retained=%lu\n",(unsigned long)custody.pending.count); fflush(stdout);
        }
        struct pollfd input={STDIN_FILENO,POLLIN,0};
        int ready=poll(&input,1,20);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0) return 2;
        if (ready>0 && (input.revents & (POLLIN|POLLHUP))) {
            char buffer[32]; ssize_t n=read(STDIN_FILENO,buffer,sizeof(buffer));
            if (n==0 || (n>=6 && !memcmp(buffer,"repair",6))) {
                TestFault=CFNone;
                if (!custodyAllowsExit(custody)) return 3;
                puts("repaired retained=0"); fflush(stdout); return 0;
            }
        }
    }
}
static GuardDelegate *Delegate;
int main(int argc,const char *argv[]) { @autoreleasepool {
    if (argc==2 && strcmp(argv[1],"--custody-self-test")==0) return custodySelfTest();
    if (argc==2 && strcmp(argv[1],"--custody-term-child")==0) return custodyTermChild();
    Env = NSProcessInfo.processInfo.environment; UserRole = [Env[@"GUARD_ROLE"] isEqual:@"user"];
    LogPath = Env[@"GUARD_LOG"]; CommandPath = Env[@"GUARD_COMMAND"];
    if (!LogPath || !CommandPath) return 2;
    [[NSUserDefaults standardUserDefaults] registerDefaults:@{@"NSMenuDisableOutOfProcessMenusDueToIncompatibility":@YES}];
    GuardApplication *app = [GuardApplication sharedApplication]; [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
    Delegate = [GuardDelegate new]; app.delegate = Delegate; [app run]; releaseLease(); return 0;
} }
