#pragma once

#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ApplicationServices/ApplicationServices.h>

NS_ASSUME_NONNULL_BEGIN

// MARK: - CGVirtualDisplay (private CoreGraphics classes)
//
// The same interface DeskPad and BetterDisplay use. A display exists for as
// long as its CGVirtualDisplay object is alive.

@class CGVirtualDisplay;

@interface CGVirtualDisplayDescriptor : NSObject
@property (retain, nonatomic, nullable) dispatch_queue_t queue;
@property (copy, nonatomic) NSString *name;
@property (nonatomic) unsigned int maxPixelsWide;
@property (nonatomic) unsigned int maxPixelsHigh;
@property (nonatomic) CGSize sizeInMillimeters;
@property (nonatomic) unsigned int serialNum;
@property (nonatomic) unsigned int productID;
@property (nonatomic) unsigned int vendorID;
@property (copy, nonatomic, nullable) void (^terminationHandler)(id _Nullable, CGVirtualDisplay *_Nullable);
@end

@interface CGVirtualDisplayMode : NSObject
@property (readonly, nonatomic) unsigned int width;
@property (readonly, nonatomic) unsigned int height;
@property (readonly, nonatomic) double refreshRate;
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)refreshRate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property (retain, nonatomic) NSArray<CGVirtualDisplayMode *> *modes;
@property (nonatomic) unsigned int hiDPI;
@end

@interface CGVirtualDisplay : NSObject
@property (readonly, nonatomic) CGDirectDisplayID displayID;
- (nullable instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@end

// MARK: - Background input (SkyLight)
//
// Posts events to one process without the HID tap, so the real cursor stays
// where the user left it. Recipe ported from trycua/cua (MIT), which follows
// yabai's focus-without-raise pattern. Every function returns false when a
// symbol does not resolve so callers can fall back to public API.

/// SLEventPostToPid. `authenticate` attaches an SLSEventAuthenticationMessage,
/// which Chromium-class targets require for keyboard events on macOS 15+.
bool SSPostEventToPid(pid_t pid, CGEventRef event, bool authenticate);

/// CGEventSetWindowLocation. Top-left origin; AppKit reads it relative to
/// the target window when routing scrolls.
bool SSEventSetWindowLocation(CGEventRef event, CGPoint point);

/// SLEventSetIntegerValueField with a raw field index.
bool SSEventSetIntegerField(CGEventRef event, uint32_t field, int64_t value);

/// Make `windowID` the input target of `pid` without raising it or switching
/// Spaces: defocus the current front process, then focus the target window.
bool SSFocusWithoutRaise(pid_t pid, CGWindowID windowID);
bool SSMakeSelfFront(CGWindowID windowID);
int SSFrontPid(void);

/// _AXUIElementGetWindow: the CGWindowID behind an AX window element.
bool SSAXWindowID(AXUIElementRef element, CGWindowID *outWindowID);

NS_ASSUME_NONNULL_END
