#import "AgentScreenPrivate.h"

#import <dlfcn.h>
#import <objc/message.h>
#import <objc/runtime.h>

typedef struct {
    uint32_t high;
    uint32_t low;
} SSProcessSerialNumber;

typedef void (*PostToPidFn)(pid_t, CGEventRef);
typedef void (*SetAuthMessageFn)(CGEventRef, id);
typedef void (*SetWindowLocationFn)(CGEventRef, CGPoint);
typedef void (*SetIntegerFieldFn)(CGEventRef, uint32_t, int64_t);
typedef uint32_t (*MainConnectionIDFn)(void);
typedef int32_t (*GetWindowOwnerFn)(uint32_t, uint32_t, uint32_t *);
typedef int32_t (*GetConnectionPSNFn)(uint32_t, SSProcessSerialNumber *);
typedef int32_t (*PostEventRecordToFn)(const SSProcessSerialNumber *, const uint8_t *);
typedef int32_t (*GetFrontProcessFn)(SSProcessSerialNumber *);
typedef int32_t (*GetProcessForPIDFn)(pid_t, SSProcessSerialNumber *);
typedef id (*AuthFactoryFn)(Class, SEL, void *, int32_t, uint32_t);
typedef AXError (*AXGetWindowFn)(AXUIElementRef, CGWindowID *);

static struct {
    PostToPidFn postToPid;
    SetAuthMessageFn setAuthMessage;
    SetWindowLocationFn setWindowLocation;
    SetIntegerFieldFn setIntegerField;
    MainConnectionIDFn mainConnectionID;
    GetWindowOwnerFn getWindowOwner;
    GetConnectionPSNFn getConnectionPSN;
    PostEventRecordToFn postEventRecordTo;
    GetFrontProcessFn getFrontProcess;
    GetProcessForPIDFn getProcessForPID;
    AXGetWindowFn axGetWindow;
} SS;

static void SSResolve(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY | RTLD_GLOBAL);
        SS.postToPid = (PostToPidFn)dlsym(RTLD_DEFAULT, "SLEventPostToPid");
        SS.setAuthMessage = (SetAuthMessageFn)dlsym(RTLD_DEFAULT, "SLEventSetAuthenticationMessage");
        SS.setWindowLocation = (SetWindowLocationFn)dlsym(RTLD_DEFAULT, "CGEventSetWindowLocation");
        SS.setIntegerField = (SetIntegerFieldFn)dlsym(RTLD_DEFAULT, "SLEventSetIntegerValueField");
        SS.mainConnectionID = (MainConnectionIDFn)dlsym(RTLD_DEFAULT, "CGSMainConnectionID");
        SS.getWindowOwner = (GetWindowOwnerFn)dlsym(RTLD_DEFAULT, "SLSGetWindowOwner");
        SS.getConnectionPSN = (GetConnectionPSNFn)dlsym(RTLD_DEFAULT, "SLSGetConnectionPSN");
        SS.postEventRecordTo = (PostEventRecordToFn)dlsym(RTLD_DEFAULT, "SLPSPostEventRecordTo");
        SS.getFrontProcess = (GetFrontProcessFn)dlsym(RTLD_DEFAULT, "_SLPSGetFrontProcess");
        SS.getProcessForPID = (GetProcessForPIDFn)dlsym(RTLD_DEFAULT, "GetProcessForPID");
        SS.axGetWindow = (AXGetWindowFn)dlsym(RTLD_DEFAULT, "_AXUIElementGetWindow");
    });
}

// __CGEvent is {CFRuntimeBase, uint32_t, SLSEventRecord *}. Probe the offsets
// cua uses so a layout change degrades to "no auth message" instead of a crash.
static void *SSEventRecord(CGEventRef event) {
    const size_t offsets[] = {24, 32, 16};
    for (size_t i = 0; i < sizeof(offsets) / sizeof(offsets[0]); i++) {
        void *record = NULL;
        memcpy(&record, (const uint8_t *)event + offsets[i], sizeof(record));
        if (record != NULL) return record;
    }
    return NULL;
}

static void SSAttachAuthMessage(pid_t pid, CGEventRef event) {
    if (SS.setAuthMessage == NULL) return;
    Class cls = objc_getClass("SLSEventAuthenticationMessage");
    SEL sel = sel_registerName("messageWithEventRecord:pid:version:");
    // The class exists on macOS 14 but the factory only appeared in 15.
    if (cls == Nil || !class_respondsToSelector(object_getClass(cls), sel)) return;
    void *record = SSEventRecord(event);
    if (record == NULL) return;
    id message = ((AuthFactoryFn)objc_msgSend)(cls, sel, record, pid, 0);
    if (message != nil) SS.setAuthMessage(event, message);
}

bool SSPostEventToPid(pid_t pid, CGEventRef event, bool authenticate) {
    SSResolve();
    if (SS.postToPid == NULL) return false;
    if (authenticate) SSAttachAuthMessage(pid, event);
    SS.postToPid(pid, event);
    return true;
}

bool SSEventSetWindowLocation(CGEventRef event, CGPoint point) {
    SSResolve();
    if (SS.setWindowLocation == NULL) return false;
    SS.setWindowLocation(event, point);
    return true;
}

bool SSEventSetIntegerField(CGEventRef event, uint32_t field, int64_t value) {
    SSResolve();
    if (SS.setIntegerField == NULL) return false;
    SS.setIntegerField(event, field, value);
    return true;
}

static bool SSProcessForWindow(CGWindowID windowID, pid_t pid, SSProcessSerialNumber *psn) {
    if (SS.mainConnectionID != NULL && SS.getWindowOwner != NULL && SS.getConnectionPSN != NULL) {
        uint32_t owner = 0;
        if (SS.getWindowOwner(SS.mainConnectionID(), windowID, &owner) == 0 && owner != 0 &&
            SS.getConnectionPSN(owner, psn) == 0) {
            return true;
        }
    }
    return SS.getProcessForPID != NULL && SS.getProcessForPID(pid, psn) == 0;
}

// 248-byte Carbon event record: an AppKit-defined event (0x0D) whose
// subtype at 0x8a is 0x01, application activated, or 0x02, deactivated.
// AppKit in the target takes 0x01 for its app becoming active and posts
// that to the app, though WindowServer's front app doesn't change (#273).
static void SSFocusRecord(uint8_t record[0xF8], CGWindowID windowID, uint8_t direction) {
    memset(record, 0, 0xF8);
    record[0x04] = 0xF8;
    record[0x08] = 0x0D;
    memcpy(&record[0x3C], &windowID, sizeof(windowID));
    record[0x8A] = direction;
}

bool SSFocusWithoutRaise(pid_t pid, CGWindowID windowID) {
    SSResolve();
    if (SS.postEventRecordTo == NULL || SS.getFrontProcess == NULL) return false;

    SSProcessSerialNumber front = {0, 0};
    SSProcessSerialNumber target = {0, 0};
    if (SS.getFrontProcess(&front) != 0) return false;
    if (!SSProcessForWindow(windowID, pid, &target)) return false;

    // The front process is the user's. Telling it its window lost focus,
    // as a raise would, ends its text input session: the input method
    // stops composing, and Chinese typing breaks until the user clicks.
    // Only the target hears about focus, and a window of the user's own
    // front app is left alone.
    if (memcmp(&front, &target, sizeof(front)) == 0) return false;
    uint8_t record[0xF8];
    SSFocusRecord(record, windowID, 0x01);
    return SS.postEventRecordTo(&target, record) == 0;
}

bool SSAXWindowID(AXUIElementRef element, CGWindowID *outWindowID) {
    SSResolve();
    if (SS.axGetWindow == NULL) return false;
    return SS.axGetWindow(element, outWindowID) == kAXErrorSuccess;
}

// The pid of WindowServer's front process, -1 if it can't say.
int SSFrontPid(void) {
    SSResolve();
    SSProcessSerialNumber front = {0, 0};
    if (SS.getFrontProcess == NULL || SS.getFrontProcess(&front) != 0) return -1;
    typedef int32_t (*GetPIDFn)(const SSProcessSerialNumber *, pid_t *);
    GetPIDFn getPID = (GetPIDFn)dlsym(RTLD_DEFAULT, "GetProcessPID");
    pid_t pid = -2;
    if (getPID != NULL) getPID(&front, &pid);
    return pid;
}

// Make this process WindowServer's front process, with `windowID` its
// key window: a click in the Viewer activates Agent Screen, a menu bar
// app, without taking that from the app that had it.
bool SSMakeSelfFront(CGWindowID windowID) {
    SSResolve();
    typedef int32_t (*SetFrontFn)(SSProcessSerialNumber *, CGWindowID, uint32_t);
    SetFrontFn setFront = (SetFrontFn)dlsym(RTLD_DEFAULT, "_SLPSSetFrontProcessWithOptions");
    SSProcessSerialNumber own = {0, 0};
    if (setFront == NULL || SS.getProcessForPID == NULL || SS.getProcessForPID(getpid(), &own) != 0) return false;
    // 0x200: user generated.
    return setFront(&own, windowID, 0x200) == 0;
}

// Make `pid` WindowServer's front process, with `windowID` its key
// window: the front hold gives the user's app the front back with their
// window key, as their click on it would.
bool SSSetFrontProcess(pid_t pid, CGWindowID windowID) {
    SSResolve();
    typedef int32_t (*SetFrontFn)(SSProcessSerialNumber *, CGWindowID, uint32_t);
    SetFrontFn setFront = (SetFrontFn)dlsym(RTLD_DEFAULT, "_SLPSSetFrontProcessWithOptions");
    SSProcessSerialNumber psn = {0, 0};
    if (setFront == NULL || SS.getProcessForPID == NULL || SS.getProcessForPID(pid, &psn) != 0) return false;
    // 0x200: user generated.
    return setFront(&psn, windowID, 0x200) == 0;
}

// How many Spaces hold `windowID`, -1 if SkyLight can't say. A window the
// app ordered out, as a closed main window it keeps, is on none; a window
// on another Space is on one the user isn't looking at.
int SSWindowSpaceCount(CGWindowID windowID) {
    SSResolve();
    typedef CFArrayRef (*CopySpacesFn)(uint32_t, int32_t, CFArrayRef);
    CopySpacesFn copySpaces = (CopySpacesFn)dlsym(RTLD_DEFAULT, "SLSCopySpacesForWindows");
    if (copySpaces == NULL || SS.mainConnectionID == NULL) return -1;
    NSArray *windows = @[@(windowID)];
    // 0x7: the current Spaces, the others, and the user's.
    CFArrayRef spaces = copySpaces(SS.mainConnectionID(), 0x7, (__bridge CFArrayRef)windows);
    if (spaces == NULL) return -1;
    int count = (int)CFArrayGetCount(spaces);
    CFRelease(spaces);
    return count;
}


// The window server's Space management mode: 1 while each display has its
// own Spaces ("Displays have separate Spaces" on), 0 while one Space spans
// every display, -1 if SkyLight can't say. Reads only.
int SSSpaceManagementMode(void) {
    SSResolve();
    typedef int32_t (*ModeFn)(uint32_t);
    ModeFn mode = (ModeFn)dlsym(RTLD_DEFAULT, "SLSGetSpaceManagementMode");
    if (mode == NULL || SS.mainConnectionID == NULL) return -1;
    return mode(SS.mainConnectionID());
}

// The display identifier of each entry of the window server's managed
// Spaces list, one per line, into `out` of `size` bytes: a display UUID
// per display with separate Spaces, one "Main" for all without. The
// number of entries, or -1 if SkyLight can't say. Reads only.
int SSManagedDisplayIdentifiers(char *out, int size) {
    SSResolve();
    typedef CFArrayRef (*CopyFn)(uint32_t);
    CopyFn copy = (CopyFn)dlsym(RTLD_DEFAULT, "SLSCopyManagedDisplaySpaces");
    if (copy == NULL || SS.mainConnectionID == NULL || out == NULL || size < 1) return -1;
    CFArrayRef list = copy(SS.mainConnectionID());
    if (list == NULL) return -1;
    NSMutableArray<NSString *> *names = [NSMutableArray array];
    for (id entry in (__bridge NSArray *)list) {
        if (![entry isKindOfClass:[NSDictionary class]]) continue;
        id name = ((NSDictionary *)entry)[@"Display Identifier"];
        [names addObject:[name isKindOfClass:[NSString class]] ? name : @""];
    }
    CFRelease(list);
    const char *joined = [[names componentsJoinedByString:@"\n"] UTF8String];
    strlcpy(out, joined ? joined : "", (size_t)size);
    return (int)names.count;
}

// The "Displays have separate Spaces" setting as written for the next
// login: com.apple.spaces spans-displays, 1 when one Space spans every
// display, 0 when not, -1 when unset (macOS's default, separate Spaces).
int SSSpansDisplaysSetting(void) {
    CFPropertyListRef value = CFPreferencesCopyAppValue(CFSTR("spans-displays"), CFSTR("com.apple.spaces"));
    if (value == NULL) return -1;
    int result = -1;
    if (CFGetTypeID(value) == CFBooleanGetTypeID()) {
        result = CFBooleanGetValue((CFBooleanRef)value) ? 1 : 0;
    } else if (CFGetTypeID(value) == CFNumberGetTypeID()) {
        int n = 0;
        if (CFNumberGetValue((CFNumberRef)value, kCFNumberIntType, &n)) result = n != 0 ? 1 : 0;
    }
    CFRelease(value);
    return result;
}

// How many Spaces hold `windowID`, as SSWindowSpaceCount, -1 if SkyLight
// can't say; and into `unshown`, how many of them no display shows now,
// -1 if SkyLight can't say. A window its app closed but keeps stays on
// the Space it was on, which a display still shows; a window on another
// Space is on one no display shows. Reads only.
int SSWindowSpaces(CGWindowID windowID, int *unshown) {
    SSResolve();
    if (unshown != NULL) *unshown = -1;
    typedef CFArrayRef (*CopySpacesFn)(uint32_t, int32_t, CFArrayRef);
    typedef CFArrayRef (*CopyManagedFn)(uint32_t);
    CopySpacesFn copySpaces = (CopySpacesFn)dlsym(RTLD_DEFAULT, "SLSCopySpacesForWindows");
    CopyManagedFn copyManaged = (CopyManagedFn)dlsym(RTLD_DEFAULT, "SLSCopyManagedDisplaySpaces");
    if (copySpaces == NULL || SS.mainConnectionID == NULL) return -1;
    NSArray *windows = @[@(windowID)];
    // 0x7: the current Spaces, the others, and the user's.
    CFArrayRef spaces = copySpaces(SS.mainConnectionID(), 0x7, (__bridge CFArrayRef)windows);
    if (spaces == NULL) return -1;
    NSArray *held = (__bridge_transfer NSArray *)spaces;
    if (unshown != NULL && copyManaged != NULL) {
        CFArrayRef managed = copyManaged(SS.mainConnectionID());
        if (managed != NULL) {
            NSMutableSet *shown = [NSMutableSet set];
            for (id entry in (__bridge NSArray *)managed) {
                if (![entry isKindOfClass:[NSDictionary class]]) continue;
                id current = ((NSDictionary *)entry)[@"Current Space"];
                if (![current isKindOfClass:[NSDictionary class]]) continue;
                id space = ((NSDictionary *)current)[@"id64"];
                if ([space isKindOfClass:[NSNumber class]]) [shown addObject:space];
            }
            CFRelease(managed);
            int count = 0;
            for (id space in held) {
                if (![shown containsObject:space]) count++;
            }
            *unshown = count;
        }
    }
    return (int)held.count;
}
